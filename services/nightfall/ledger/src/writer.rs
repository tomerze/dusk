use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::sync::oneshot;
use tracing::{error, info};

use crate::builder::Builder;
use crate::chain::{Chain, ChainError, Continuation};
use crate::entry::{EntryContent, InvalidEntry};
use crate::log::{LedgerLog, LogError, LogErrorKind};
use crate::signing::CheckpointSigner;

#[derive(Debug, Clone)]
pub struct LedgerConfig {
    pub instance: String,
    pub partition: u32,
    pub queue_entries: usize,
    pub denial_entries: usize,
    pub session_entries: usize,
    pub checkpoint_interval: Duration,
    pub transaction_entries: usize,
    pub transaction_time: Duration,
    pub write_ahead_budget: Duration,
}

impl LedgerConfig {
    pub fn new(instance: &str, partition: u32) -> LedgerConfig {
        LedgerConfig {
            instance: String::from(instance),
            partition,
            queue_entries: 100_000,
            denial_entries: 1000,
            session_entries: 200_000,
            checkpoint_interval: Duration::from_millis(1000),
            transaction_entries: 500,
            transaction_time: Duration::from_millis(5),
            write_ahead_budget: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerStatus {
    Ready,
    Recovering,
    Fenced,
    Stopped,
}

impl LedgerStatus {
    fn from_code(code: u8) -> LedgerStatus {
        match code {
            0 => LedgerStatus::Ready,
            1 => LedgerStatus::Recovering,
            2 => LedgerStatus::Fenced,
            _ => LedgerStatus::Stopped,
        }
    }

    fn code(self) -> u8 {
        match self {
            LedgerStatus::Ready => 0,
            LedgerStatus::Recovering => 1,
            LedgerStatus::Fenced => 2,
            LedgerStatus::Stopped => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    #[error("the ledger queue is full")]
    Full,
    #[error("unavailable: ledger")]
    Unavailable,
    #[error("the ledger was fenced by a newer producer")]
    Fenced,
    #[error("the ledger has stopped")]
    Stopped,
    #[error("the reservation has no slot left")]
    NoSlot,
    #[error(transparent)]
    Invalid(#[from] InvalidEntry),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitFailure {
    #[error("the ledger did not commit within the write-ahead budget")]
    TimedOut,
    #[error("unavailable: ledger")]
    Unavailable,
    #[error("the ledger was fenced by a newer producer")]
    Fenced,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("reading the ledger partition tail failed: {0}")]
    Log(#[from] LogError),
    #[error(transparent)]
    Chain(#[from] ChainError),
    #[error("starting the ledger chain builder thread failed: {0}")]
    Thread(#[from] std::io::Error),
}

pub trait LedgerObserver: Send + Sync {
    fn committed(&self, _entries: usize, _duration: Duration) {}
    fn delivery_failed(&self, _kind: LogErrorKind) {}
    fn status_changed(&self, _status: LedgerStatus) {}
}

pub struct NoObserver;

impl LedgerObserver for NoObserver {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pool {
    Main,
    Denial,
    Session,
}

pub(crate) type CommitSender = oneshot::Sender<Result<(), CommitFailure>>;

pub(crate) struct Queued {
    pub id: String,
    pub time: String,
    pub content: EntryContent,
    pub commit: Option<CommitSender>,
    pub pool: Pool,
}

pub(crate) enum Message {
    Entry(Box<Queued>),
    Shutdown,
}

pub(crate) struct Shared {
    pub config: LedgerConfig,
    main_used: AtomicUsize,
    denial_used: AtomicUsize,
    session_used: AtomicUsize,
    queued: AtomicUsize,
    senders: AtomicUsize,
    status: AtomicU8,
    sender: SyncSender<Message>,
}

impl Shared {
    fn try_take(&self, pool: Pool, slots: usize) -> bool {
        let (counter, limit) = match pool {
            Pool::Main => (&self.main_used, self.config.queue_entries),
            Pool::Denial => (&self.denial_used, self.config.denial_entries),
            Pool::Session => (&self.session_used, self.config.session_entries),
        };
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(slots).filter(|total| *total <= limit)
            })
            .is_ok()
    }

    pub(crate) fn release(&self, pool: Pool, slots: usize) {
        let counter = match pool {
            Pool::Main => &self.main_used,
            Pool::Denial => &self.denial_used,
            Pool::Session => &self.session_used,
        };
        counter.fetch_sub(slots, Ordering::AcqRel);
    }

    pub(crate) fn settle(&self, pool: Pool) {
        self.release(pool, 1);
        self.queued.fetch_sub(1, Ordering::AcqRel);
    }

    pub(crate) fn status(&self) -> LedgerStatus {
        LedgerStatus::from_code(self.status.load(Ordering::SeqCst))
    }

    pub(crate) fn set_status(&self, status: LedgerStatus) {
        self.status.store(status.code(), Ordering::SeqCst);
    }

    pub(crate) fn transition(&self, from: LedgerStatus, to: LedgerStatus) -> bool {
        self.status
            .compare_exchange(from.code(), to.code(), Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub(crate) fn wait_for_senders(&self) {
        while self.senders.load(Ordering::SeqCst) > 0 {
            std::thread::yield_now();
        }
    }

    fn refusal(&self) -> Option<Refused> {
        match self.status() {
            LedgerStatus::Ready => None,
            LedgerStatus::Recovering => Some(Refused::Unavailable),
            LedgerStatus::Fenced => Some(Refused::Fenced),
            LedgerStatus::Stopped => Some(Refused::Stopped),
        }
    }
}

#[derive(Clone)]
pub struct LedgerWriter {
    shared: Arc<Shared>,
}

pub struct LedgerThread {
    handle: JoinHandle<()>,
}

impl LedgerThread {
    pub fn join(self) {
        if self.handle.join().is_err() {
            error!("the ledger chain builder thread panicked");
        }
    }
}

impl LedgerWriter {
    pub fn start(
        config: LedgerConfig,
        mut log: Box<dyn LedgerLog>,
        signer: CheckpointSigner,
        observer: Arc<dyn LedgerObserver>,
    ) -> Result<(LedgerWriter, LedgerThread), LedgerError> {
        let tail = log.read_tail(1)?;
        let (chain, opening) = Chain::start(
            &config.instance,
            config.partition,
            tail.last().map(Vec::as_slice),
            Continuation::Start,
        )?;
        let position = chain.position();
        info!(
            instance = %config.instance,
            partition = config.partition,
            sequence = position.as_ref().map(|position| position.sequence),
            linked = opening.is_some(),
            key_id = signer.key_id(),
            "ledger chain started"
        );
        let (sender, receiver) = std::sync::mpsc::sync_channel(
            config.queue_entries + config.denial_entries + config.session_entries + 1,
        );
        let shared = Arc::new(Shared {
            config,
            main_used: AtomicUsize::new(0),
            denial_used: AtomicUsize::new(0),
            session_used: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            senders: AtomicUsize::new(0),
            status: AtomicU8::new(LedgerStatus::Ready.code()),
            sender,
        });
        let builder = Builder::new(
            shared.clone(),
            receiver,
            log,
            chain,
            opening,
            signer,
            observer,
        );
        let handle = std::thread::Builder::new()
            .name(String::from("ledger-chain"))
            .spawn(move || builder.run())?;
        Ok((LedgerWriter { shared }, LedgerThread { handle }))
    }

    pub fn reserve(&self, slots: u32) -> Result<Reservation, Refused> {
        self.reserve_from(Pool::Main, slots)
    }

    pub fn reserve_denial(&self) -> Result<Reservation, Refused> {
        self.reserve_from(Pool::Denial, 1)
    }

    pub fn reserve_session(&self) -> Result<Reservation, Refused> {
        self.reserve_from(Pool::Session, 2)
    }

    fn reserve_from(&self, pool: Pool, slots: u32) -> Result<Reservation, Refused> {
        if let Some(refusal) = self.shared.refusal() {
            return Err(refusal);
        }
        if !self.shared.try_take(pool, slots as usize) {
            return Err(Refused::Full);
        }
        Ok(Reservation {
            shared: self.shared.clone(),
            pool,
            slots,
        })
    }

    pub fn status(&self) -> LedgerStatus {
        self.shared.status()
    }

    pub fn queue_depth(&self) -> usize {
        self.shared.queued.load(Ordering::Acquire)
    }

    pub fn shutdown(&self) {
        let previous = self.shared.status();
        if matches!(previous, LedgerStatus::Fenced | LedgerStatus::Stopped) {
            return;
        }
        self.shared.set_status(LedgerStatus::Stopped);
        if self.shared.sender.send(Message::Shutdown).is_err() {
            error!("the ledger chain builder had already exited when shutdown was requested");
        }
    }
}

pub struct Reservation {
    shared: Arc<Shared>,
    pool: Pool,
    slots: u32,
}

impl Reservation {
    pub fn slots(&self) -> u32 {
        self.slots
    }

    pub fn split(&mut self, slots: u32) -> Result<Reservation, Refused> {
        if slots > self.slots {
            return Err(Refused::NoSlot);
        }
        self.slots -= slots;
        Ok(Reservation {
            shared: self.shared.clone(),
            pool: self.pool,
            slots,
        })
    }

    pub fn record(&mut self, entry: EntryContent) -> Result<(), Refused> {
        self.enqueue(entry, None)
    }

    pub fn record_write_ahead(&mut self, entry: EntryContent) -> Result<Commit, Refused> {
        let (sender, receiver) = oneshot::channel();
        let deadline = tokio::time::Instant::now() + self.shared.config.write_ahead_budget;
        self.enqueue(entry, Some(sender))?;
        Ok(Commit(Box::pin(async move {
            match tokio::time::timeout_at(deadline, receiver).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(_)) => Err(CommitFailure::Unavailable),
                Err(_) => Err(CommitFailure::TimedOut),
            }
        })))
    }

    fn enqueue(
        &mut self,
        content: EntryContent,
        commit: Option<CommitSender>,
    ) -> Result<(), Refused> {
        if self.slots == 0 {
            return Err(Refused::NoSlot);
        }
        content.validate()?;
        let _sending = Sending::enter(&self.shared);
        match self.shared.status() {
            LedgerStatus::Fenced => return Err(Refused::Fenced),
            LedgerStatus::Stopped => return Err(Refused::Stopped),
            LedgerStatus::Ready | LedgerStatus::Recovering => {}
        }
        let queued = Queued {
            id: uuid::Uuid::now_v7().to_string(),
            time: crate::time::now(),
            content,
            commit,
            pool: self.pool,
        };
        self.shared.queued.fetch_add(1, Ordering::AcqRel);
        match self
            .shared
            .sender
            .try_send(Message::Entry(Box::new(queued)))
        {
            Ok(()) => {
                self.slots -= 1;
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                self.shared.queued.fetch_sub(1, Ordering::AcqRel);
                error!("the ledger channel is full although every entry holds a reserved slot");
                Err(Refused::Full)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.queued.fetch_sub(1, Ordering::AcqRel);
                Err(Refused::Stopped)
            }
        }
    }
}

struct Sending<'a>(&'a Shared);

impl<'a> Sending<'a> {
    fn enter(shared: &'a Shared) -> Sending<'a> {
        shared.senders.fetch_add(1, Ordering::SeqCst);
        Sending(shared)
    }
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        self.0.senders.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.slots > 0 {
            self.shared.release(self.pool, self.slots as usize);
        }
    }
}

pub struct Commit(Pin<Box<dyn Future<Output = Result<(), CommitFailure>> + Send>>);

impl Future for Commit {
    type Output = Result<(), CommitFailure>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.as_mut().poll(context)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Instant;

    use serde_json::Value;

    use super::*;
    use crate::entry::ResultCode;
    use crate::entry::fixtures::{call, result, session_open};
    use crate::log::LogErrorKind;
    use crate::memory::{Fault, MemoryLog};
    use crate::signing::VerifyingKeys;
    use crate::signing::fixtures::signer;
    use crate::verifier::{VerificationReport, Verifier, VerifyOptions};

    const INSTANCE: &str = "nightfall-0";

    #[derive(Default)]
    struct Recording {
        statuses: Mutex<Vec<LedgerStatus>>,
        failures: Mutex<Vec<LogErrorKind>>,
    }

    impl LedgerObserver for Recording {
        fn delivery_failed(&self, kind: LogErrorKind) {
            self.failures.lock().unwrap().push(kind);
        }

        fn status_changed(&self, status: LedgerStatus) {
            self.statuses.lock().unwrap().push(status);
        }
    }

    fn quiet_config() -> LedgerConfig {
        LedgerConfig {
            checkpoint_interval: Duration::from_secs(3600),
            ..LedgerConfig::new(INSTANCE, 0)
        }
    }

    fn start(
        config: LedgerConfig,
        log: &MemoryLog,
    ) -> (LedgerWriter, LedgerThread, Arc<Recording>) {
        let observer = Arc::new(Recording::default());
        let (writer, thread) =
            LedgerWriter::start(config, Box::new(log.clone()), signer(5), observer.clone())
                .unwrap();
        (writer, thread, observer)
    }

    fn keys() -> VerifyingKeys {
        VerifyingKeys::from_jwks(&signer(5).public_jwks().to_string()).unwrap()
    }

    fn verify(log: &MemoryLog) -> VerificationReport {
        let mut verifier = Verifier::new(keys(), VerifyOptions::default());
        for record in log.records() {
            verifier.push_record(&record);
        }
        verifier.finish()
    }

    fn entries(log: &MemoryLog) -> Vec<Value> {
        log.records()
            .iter()
            .map(|record| serde_json::from_slice(record).unwrap())
            .collect()
    }

    fn stop(writer: &LedgerWriter, thread: LedgerThread) {
        writer.shutdown();
        thread.join();
    }

    #[tokio::test]
    async fn commits_a_write_ahead_call_and_its_result_and_signs_the_tail_on_shutdown() {
        let log = MemoryLog::new();
        let (writer, thread, observer) = start(quiet_config(), &log);
        let mut reservation = writer.reserve(2).unwrap();
        reservation
            .record_write_ahead(call())
            .unwrap()
            .await
            .unwrap();
        assert_eq!(log.records().len(), 1);
        reservation.record(result()).unwrap();
        assert_eq!(reservation.slots(), 0);
        stop(&writer, thread);
        let written = entries(&log);
        let kinds: Vec<&str> = written
            .iter()
            .map(|entry| entry["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["call", "result", "checkpoint"]);
        assert_eq!(written[1]["call_id"], written[0]["call_id"]);
        let report = verify(&log);
        assert!(report.is_clean(), "{report}");
        assert_eq!(report.chains[0].unsigned_tail, 0);
        assert_eq!(writer.status(), LedgerStatus::Stopped);
        assert_eq!(writer.queue_depth(), 0);
        assert_eq!(*observer.statuses.lock().unwrap(), [LedgerStatus::Stopped]);
        assert_eq!(writer.reserve(1).err(), Some(Refused::Stopped));
    }

    #[test]
    fn bounds_each_transaction_by_entries() {
        let log = MemoryLog::new();
        let config = LedgerConfig {
            transaction_entries: 3,
            transaction_time: Duration::from_secs(3600),
            ..quiet_config()
        };
        let (writer, thread, _) = start(config, &log);
        let mut reservation = writer.reserve(10).unwrap();
        for _ in 0..10 {
            reservation.record(session_open()).unwrap();
        }
        stop(&writer, thread);
        assert_eq!(log.records().len(), 11);
        assert!(log.commits() >= 4, "{} commits", log.commits());
        assert!(verify(&log).is_clean());
    }

    #[test]
    fn closes_a_transaction_when_its_time_is_up() {
        let log = MemoryLog::new();
        let config = LedgerConfig {
            transaction_time: Duration::from_millis(5),
            ..quiet_config()
        };
        let (writer, thread, _) = start(config, &log);
        let mut reservation = writer.reserve(2).unwrap();
        reservation.record(session_open()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while log.records().is_empty() {
            assert!(
                Instant::now() < deadline,
                "the first transaction never committed"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        reservation.record(session_open()).unwrap();
        stop(&writer, thread);
        assert!(log.commits() >= 2);
    }

    #[test]
    fn reserves_capacity_and_keeps_a_separate_denial_pool() {
        let log = MemoryLog::new();
        let config = LedgerConfig {
            queue_entries: 4,
            denial_entries: 2,
            ..quiet_config()
        };
        let (writer, thread, _) = start(config, &log);
        let first = writer.reserve(2).unwrap();
        let mut second = writer.reserve(2).unwrap();
        assert_eq!(writer.reserve(1).err(), Some(Refused::Full));
        let denial = writer.reserve_denial().unwrap();
        let other_denial = writer.reserve_denial().unwrap();
        assert_eq!(writer.reserve_denial().err(), Some(Refused::Full));
        drop(first);
        let mut third = writer.reserve(2).unwrap();
        let mut split = third.split(1).unwrap();
        assert_eq!((third.slots(), split.slots()), (1, 1));
        assert_eq!(split.split(2).err(), Some(Refused::NoSlot));
        split.record(session_open()).unwrap();
        assert_eq!(split.record(session_open()).err(), Some(Refused::NoSlot));
        second.record(session_open()).unwrap();
        drop((second, third, denial, other_denial));
        stop(&writer, thread);
        assert_eq!(log.records().len(), 3);
    }

    #[test]
    fn sessions_take_their_open_and_close_entries_from_their_own_pool() {
        let log = MemoryLog::new();
        let config = LedgerConfig {
            queue_entries: 2,
            session_entries: 4,
            ..quiet_config()
        };
        let (writer, thread, _) = start(config, &log);
        let mut first = writer.reserve_session().unwrap();
        let _second = writer.reserve_session().unwrap();
        assert_eq!(writer.reserve_session().err(), Some(Refused::Full));
        let mut call = writer.reserve(2).unwrap();
        call.record(session_open()).unwrap();
        first.record(session_open()).unwrap();
        stop(&writer, thread);
        assert_eq!(log.records().len(), 3);
    }

    #[test]
    fn writes_denied_calls_from_the_denial_pool() {
        let log = MemoryLog::new();
        let config = LedgerConfig {
            queue_entries: 0,
            ..quiet_config()
        };
        let (writer, thread, _) = start(config, &log);
        assert_eq!(writer.reserve(2).err(), Some(Refused::Full));
        let mut denial = writer.reserve_denial().unwrap();
        denial
            .record(EntryContent {
                result_code: Some(ResultCode::Denied),
                param_hash: String::new(),
                ..call()
            })
            .unwrap();
        stop(&writer, thread);
        assert_eq!(entries(&log)[0]["result_code"], "denied");
    }

    #[test]
    fn refuses_an_invalid_entry_without_spending_its_slot() {
        let log = MemoryLog::new();
        let (writer, thread, _) = start(quiet_config(), &log);
        let mut reservation = writer.reserve(1).unwrap();
        let refused = reservation.record(EntryContent {
            session_id: None,
            ..call()
        });
        assert!(matches!(refused, Err(Refused::Invalid(_))));
        assert_eq!(reservation.slots(), 1);
        drop(reservation);
        stop(&writer, thread);
        assert!(log.records().is_empty());
    }

    #[test]
    fn retries_a_failed_transaction_without_duplicating_entries() {
        let log = MemoryLog::new();
        log.inject(Fault::Commit(LogErrorKind::Retriable));
        log.inject(Fault::Append(LogErrorKind::Abortable));
        let (writer, thread, observer) = start(quiet_config(), &log);
        let mut reservation = writer.reserve(2).unwrap();
        reservation.record(call()).unwrap();
        reservation.record(result()).unwrap();
        stop(&writer, thread);
        assert_eq!(
            *observer.failures.lock().unwrap(),
            [LogErrorKind::Retriable, LogErrorKind::Abortable]
        );
        let report = verify(&log);
        assert!(report.is_clean(), "{report}");
        assert_eq!(report.entries, 3);
        assert_eq!(log.recoveries(), 0);
    }

    #[tokio::test]
    async fn recovers_from_a_fatal_error_after_the_commit_applied() {
        let log = MemoryLog::new();
        let (writer, thread, observer) = start(quiet_config(), &log);
        let mut reservation = writer.reserve(2).unwrap();
        reservation
            .record_write_ahead(call())
            .unwrap()
            .await
            .unwrap();
        log.inject(Fault::CommitAppliedThen(LogErrorKind::Fatal));
        reservation
            .record_write_ahead(result())
            .unwrap()
            .await
            .unwrap();
        stop(&writer, thread);
        assert_eq!(log.recoveries(), 1);
        let written = entries(&log);
        let events: Vec<Option<&str>> = written
            .iter()
            .map(|entry| entry["event"].as_str())
            .collect();
        assert_eq!(events, [None, None, Some("chain_resumed"), None]);
        assert_eq!(written[2]["event_detail"]["sequence"], 1);
        assert_eq!(written[2]["event_detail"]["hash"], written[1]["hash"]);
        let report = verify(&log);
        assert!(report.is_clean(), "{report}");
        assert_eq!(
            *observer.statuses.lock().unwrap(),
            [
                LedgerStatus::Recovering,
                LedgerStatus::Ready,
                LedgerStatus::Stopped
            ]
        );
    }

    #[tokio::test]
    async fn rechains_unwritten_entries_after_a_fatal_error() {
        let log = MemoryLog::new();
        let (writer, thread, _) = start(quiet_config(), &log);
        let mut reservation = writer.reserve(3).unwrap();
        reservation
            .record_write_ahead(session_open())
            .unwrap()
            .await
            .unwrap();
        log.inject(Fault::Commit(LogErrorKind::Fatal));
        log.inject(Fault::Recover(LogErrorKind::Retriable));
        reservation
            .record_write_ahead(call())
            .unwrap()
            .await
            .unwrap();
        reservation.record(result()).unwrap();
        stop(&writer, thread);
        let written = entries(&log);
        let shapes: Vec<(&str, Option<&str>)> = written
            .iter()
            .map(|entry| (entry["kind"].as_str().unwrap(), entry["event"].as_str()))
            .collect();
        assert_eq!(
            shapes,
            [
                ("event", Some("session_open")),
                ("event", Some("chain_resumed")),
                ("call", None),
                ("result", None),
                ("checkpoint", None)
            ]
        );
        assert_eq!(log.recoveries(), 1);
        let report = verify(&log);
        assert!(report.is_clean(), "{report}");
    }

    #[tokio::test]
    async fn stops_forwarding_when_fenced() {
        let log = MemoryLog::new();
        log.inject(Fault::Commit(LogErrorKind::Fenced));
        let (writer, thread, observer) = start(quiet_config(), &log);
        let mut reservation = writer.reserve(2).unwrap();
        let commit = reservation.record_write_ahead(call()).unwrap();
        assert_eq!(commit.await, Err(CommitFailure::Fenced));
        thread.join();
        assert_eq!(writer.status(), LedgerStatus::Fenced);
        assert_eq!(reservation.record(result()).err(), Some(Refused::Fenced));
        assert_eq!(writer.reserve(1).err(), Some(Refused::Fenced));
        assert_eq!(writer.queue_depth(), 0);
        assert!(log.records().is_empty());
        assert_eq!(*observer.statuses.lock().unwrap(), [LedgerStatus::Fenced]);
        writer.shutdown();
    }

    #[tokio::test]
    async fn fails_a_write_ahead_call_that_misses_its_budget() {
        let log = MemoryLog::new();
        for _ in 0..4 {
            log.inject(Fault::Commit(LogErrorKind::Retriable));
        }
        let config = LedgerConfig {
            write_ahead_budget: Duration::from_millis(1),
            ..quiet_config()
        };
        let (writer, thread, _) = start(config, &log);
        let mut reservation = writer.reserve(1).unwrap();
        let commit = reservation.record_write_ahead(call()).unwrap();
        assert_eq!(commit.await, Err(CommitFailure::TimedOut));
        stop(&writer, thread);
        assert_eq!(entries(&log)[0]["kind"], "call");
    }

    #[test]
    fn continues_its_own_chain_and_links_another_instances_tail() {
        let log = MemoryLog::new();
        let (writer, thread, _) = start(quiet_config(), &log);
        writer.reserve(1).unwrap().record(session_open()).unwrap();
        stop(&writer, thread);

        let (writer, thread, _) = start(quiet_config(), &log);
        writer.reserve(1).unwrap().record(session_open()).unwrap();
        stop(&writer, thread);
        let written = entries(&log);
        assert_eq!(written.len(), 4);
        assert_eq!(written[2]["sequence"], 2);
        assert_eq!(written[2]["previous_hash"], written[1]["hash"]);

        let config = LedgerConfig {
            instance: String::from("nightfall-0-replacement"),
            ..quiet_config()
        };
        let (writer, thread, _) = start(config, &log);
        stop(&writer, thread);
        let written = entries(&log);
        let link = &written[4];
        assert_eq!(link["event"], "chain_link");
        assert_eq!(link["sequence"], 0);
        assert_eq!(link["event_detail"]["instance"], INSTANCE);
        assert_eq!(link["event_detail"]["sequence"], 3);
        assert_eq!(written[5]["kind"], "checkpoint");
        let report = verify(&log);
        assert!(report.is_clean(), "{report}");
        assert_eq!(report.chains.len(), 2);
    }

    #[test]
    fn writes_checkpoints_on_its_interval() {
        let log = MemoryLog::new();
        let config = LedgerConfig {
            checkpoint_interval: Duration::from_millis(10),
            ..quiet_config()
        };
        let (writer, thread, _) = start(config, &log);
        writer.reserve(1).unwrap().record(session_open()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while entries(&log)
            .iter()
            .all(|entry| entry["kind"] != "checkpoint")
        {
            assert!(Instant::now() < deadline, "no checkpoint was written");
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(Duration::from_millis(50));
        let checkpoints = entries(&log)
            .iter()
            .filter(|entry| entry["kind"] == "checkpoint")
            .count();
        assert_eq!(checkpoints, 1);
        stop(&writer, thread);
        assert!(verify(&log).is_clean());
        assert_eq!(entries(&log).len(), 2);
    }
}
