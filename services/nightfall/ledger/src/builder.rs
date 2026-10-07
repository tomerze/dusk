use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use tracing::{debug, error, info, info_span, warn};

use crate::chain::{Chain, ChainError, Continuation};
use crate::entry::{EntryContent, LedgerEntry};
use crate::log::{LedgerLog, LogError, LogErrorKind};
use crate::signing::CheckpointSigner;
use crate::writer::{
    CommitFailure, CommitSender, LedgerObserver, LedgerStatus, Message, Pool, Queued, Shared,
};

const BACKOFF_BASE: Duration = Duration::from_millis(100);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

struct Pending {
    id: String,
    time: String,
    content: Option<EntryContent>,
    payload: Vec<u8>,
    sequence: u64,
    commit: Option<CommitSender>,
    pool: Option<Pool>,
}

pub(crate) struct Builder {
    shared: Arc<Shared>,
    receiver: Receiver<Message>,
    log: Box<dyn LedgerLog>,
    chain: Chain,
    signer: CheckpointSigner,
    observer: Arc<dyn LedgerObserver>,
    batch: Vec<Pending>,
    checkpoint_due: Instant,
    stopping: bool,
}

enum Outcome {
    Committed,
    Halted,
}

impl Builder {
    pub(crate) fn new(
        shared: Arc<Shared>,
        receiver: Receiver<Message>,
        log: Box<dyn LedgerLog>,
        chain: Chain,
        opening: Option<EntryContent>,
        signer: CheckpointSigner,
        observer: Arc<dyn LedgerObserver>,
    ) -> Builder {
        let checkpoint_due = Instant::now() + shared.config.checkpoint_interval;
        let mut builder = Builder {
            shared,
            receiver,
            log,
            chain,
            signer,
            observer,
            batch: Vec::new(),
            checkpoint_due,
            stopping: false,
        };
        if let Some(opening) = opening {
            builder.push_generated(opening);
        }
        builder
    }

    pub(crate) fn run(mut self) {
        let span = info_span!(
            "ledger_chain",
            instance = %self.shared.config.instance,
            partition = self.shared.config.partition
        );
        let _entered = span.enter();
        loop {
            self.collect();
            let now = Instant::now();
            if now >= self.checkpoint_due || self.stopping {
                self.checkpoint();
                self.checkpoint_due = now + self.shared.config.checkpoint_interval;
            }
            if !self.batch.is_empty() && matches!(self.commit_batch(), Outcome::Halted) {
                return;
            }
            if self.stopping && self.batch.is_empty() && self.finish() {
                return;
            }
        }
    }

    fn collect(&mut self) {
        let mut opened = (!self.batch.is_empty()).then(Instant::now);
        while self.batch.len() < self.shared.config.transaction_entries {
            let message = if self.stopping {
                match self.receiver.try_recv() {
                    Ok(message) => message,
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return,
                }
            } else {
                let wait = match opened {
                    Some(opened) => {
                        let wait = (opened + self.shared.config.transaction_time)
                            .saturating_duration_since(Instant::now());
                        if wait.is_zero() {
                            return;
                        }
                        wait
                    }
                    None => self
                        .checkpoint_due
                        .saturating_duration_since(Instant::now()),
                };
                match self.receiver.recv_timeout(wait) {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout) => return,
                    Err(RecvTimeoutError::Disconnected) => {
                        self.stopping = true;
                        return;
                    }
                }
            };
            match message {
                Message::Entry(queued) => {
                    self.push(*queued);
                    opened.get_or_insert_with(Instant::now);
                }
                Message::Shutdown => {
                    info!(
                        "ledger shutdown requested; writing the queued entries and a final checkpoint"
                    );
                    self.stopping = true;
                }
            }
        }
    }

    fn finish(&mut self) -> bool {
        self.shared.wait_for_senders();
        let mut stragglers = false;
        while let Ok(message) = self.receiver.try_recv() {
            if let Message::Entry(queued) = message {
                self.push(*queued);
                stragglers = true;
            }
        }
        if stragglers {
            return false;
        }
        let position = self.chain.position();
        info!(
            sequence = position.as_ref().map(|position| position.sequence),
            hash = position.as_ref().map(|position| position.hash.as_str()),
            "ledger chain stopped"
        );
        self.observer.status_changed(LedgerStatus::Stopped);
        true
    }

    fn push(&mut self, queued: Queued) {
        let Queued {
            id,
            time,
            content,
            commit,
            pool,
        } = queued;
        match self.chain.append(id.clone(), time.clone(), content.clone()) {
            Ok(entry) => match entry.payload() {
                Ok(payload) => self.batch.push(Pending {
                    id,
                    time,
                    content: Some(content),
                    payload,
                    sequence: entry.sequence,
                    commit,
                    pool: Some(pool),
                }),
                Err(failure) => self.reject(id, commit, pool, &failure.to_string()),
            },
            Err(failure) => self.reject(id, commit, pool, &failure.to_string()),
        }
    }

    fn reject(&mut self, id: String, commit: Option<CommitSender>, pool: Pool, reason: &str) {
        error!(entry_id = %id, reason, "ledger entry cannot be canonicalized and is not written");
        if let Some(commit) = commit
            && commit.send(Err(CommitFailure::Unavailable)).is_err()
        {
            warn!(entry_id = %id, "the caller of a rejected write-ahead entry had stopped waiting");
        }
        self.shared.settle(pool);
    }

    fn push_generated(&mut self, content: EntryContent) {
        let id = uuid::Uuid::now_v7().to_string();
        match self.chain.append(id.clone(), crate::time::now(), content) {
            Ok(entry) => self.push_entry(entry),
            Err(failure) => {
                error!(entry_id = %id, error = %failure, "ledger event cannot be canonicalized")
            }
        }
    }

    fn push_entry(&mut self, entry: LedgerEntry) {
        match entry.payload() {
            Ok(payload) => self.batch.push(Pending {
                id: entry.id,
                time: entry.time,
                content: None,
                payload,
                sequence: entry.sequence,
                commit: None,
                pool: None,
            }),
            Err(failure) => {
                error!(entry_id = %entry.id, error = %failure, "ledger entry cannot be canonicalized")
            }
        }
    }

    fn checkpoint(&mut self) {
        match self.chain.checkpoint(
            &self.signer,
            uuid::Uuid::now_v7().to_string(),
            crate::time::now(),
        ) {
            Ok(Some(entry)) => {
                debug!(sequence = entry.sequence, "ledger checkpoint");
                self.push_entry(entry);
            }
            Ok(None) => {}
            Err(failure) => error!(error = %failure, "ledger checkpoint cannot be canonicalized"),
        }
    }

    fn commit_batch(&mut self) -> Outcome {
        let started = Instant::now();
        let mut attempt = 0u32;
        loop {
            let error = match self.write_transaction() {
                Ok(()) => {
                    self.settle_batch(started);
                    return Outcome::Committed;
                }
                Err(error) => error,
            };
            self.observer.delivery_failed(error.kind);
            match error.kind {
                LogErrorKind::Retriable | LogErrorKind::Abortable => {
                    warn!(error = %error, entries = self.batch.len(), attempt, "ledger transaction failed; aborting it to retry");
                    if let Err(abort_error) = self.log.abort() {
                        match abort_error.kind {
                            LogErrorKind::Fenced => return self.fence(&abort_error),
                            LogErrorKind::Fatal => {
                                if !self.recover(&abort_error) {
                                    return Outcome::Halted;
                                }
                                attempt = 0;
                                continue;
                            }
                            LogErrorKind::Retriable | LogErrorKind::Abortable => {
                                warn!(error = %abort_error, "aborting the failed ledger transaction failed")
                            }
                        }
                    }
                    backoff(attempt);
                    attempt = attempt.saturating_add(1);
                }
                LogErrorKind::Fatal => {
                    if !self.recover(&error) {
                        return Outcome::Halted;
                    }
                    attempt = 0;
                }
                LogErrorKind::Fenced => return self.fence(&error),
            }
        }
    }

    fn write_transaction(&mut self) -> Result<(), LogError> {
        self.log.begin()?;
        for pending in &self.batch {
            self.log.append(&pending.payload)?;
        }
        self.log.commit()
    }

    fn settle_batch(&mut self, started: Instant) {
        let entries = self.batch.len();
        let last_sequence = self.batch.last().map(|pending| pending.sequence);
        for pending in std::mem::take(&mut self.batch) {
            self.settle(pending);
        }
        let duration = started.elapsed();
        debug!(
            entries,
            sequence = last_sequence,
            duration_seconds = duration.as_secs_f64(),
            "ledger transaction committed"
        );
        self.observer.committed(entries, duration);
    }

    fn settle(&self, pending: Pending) {
        if let Some(commit) = pending.commit
            && commit.send(Ok(())).is_err()
        {
            warn!(
                entry_id = %pending.id,
                sequence = pending.sequence,
                "write-ahead ledger entry committed after its caller stopped waiting; the call was not forwarded"
            );
        }
        if let Some(pool) = pending.pool {
            self.shared.settle(pool);
        }
    }

    fn recover(&mut self, cause: &LogError) -> bool {
        error!(error = %cause, entries = self.batch.len(), "ledger producer failed fatally; recovering");
        if self
            .shared
            .transition(LedgerStatus::Ready, LedgerStatus::Recovering)
        {
            self.observer.status_changed(LedgerStatus::Recovering);
        }
        let mut attempt = 0u32;
        let committed = loop {
            if attempt > 0 && self.shared.status() == LedgerStatus::Stopped {
                let lost = self.abandon(CommitFailure::Unavailable);
                error!(
                    lost,
                    "ledger shutdown requested while the producer was still recovering; the queued entries are not written"
                );
                return false;
            }
            backoff(attempt);
            attempt = attempt.saturating_add(1);
            if let Err(error) = self.log.recover() {
                if error.kind == LogErrorKind::Fenced {
                    self.fence(&error);
                    return false;
                }
                warn!(error = %error, attempt, "recreating the ledger producer failed");
                continue;
            }
            let tail = match self.log.read_tail(self.batch.len() + 1) {
                Ok(tail) => tail,
                Err(error) => {
                    if error.kind == LogErrorKind::Fenced {
                        self.fence(&error);
                        return false;
                    }
                    warn!(error = %error, attempt, "reading the ledger partition tail failed");
                    continue;
                }
            };
            match self.rebuild(&tail) {
                Ok(committed) => break committed,
                Err(error) => {
                    error!(error = %error, attempt, "the ledger chain cannot resume from the partition tail")
                }
            }
        };
        if self
            .shared
            .transition(LedgerStatus::Recovering, LedgerStatus::Ready)
        {
            self.observer.status_changed(LedgerStatus::Ready);
        }
        for pending in committed {
            self.settle(pending);
        }
        true
    }

    fn rebuild(&mut self, tail: &[Vec<u8>]) -> Result<Vec<Pending>, ChainError> {
        let (chain, opening) = Chain::start(
            &self.shared.config.instance,
            self.shared.config.partition,
            tail.last().map(Vec::as_slice),
            Continuation::Resume,
        )?;
        let present: HashSet<String> = tail
            .iter()
            .filter_map(|record| {
                let value: Value = serde_json::from_slice(record).ok()?;
                value.get("id").and_then(Value::as_str).map(String::from)
            })
            .collect();
        let mut committed = Vec::new();
        let mut rechain = Vec::new();
        for pending in std::mem::take(&mut self.batch) {
            if present.contains(&pending.id) {
                committed.push(pending);
            } else if pending.content.is_some() {
                rechain.push(pending);
            }
        }
        if tail.is_empty() {
            warn!(
                "the ledger partition is empty after recovery; starting a new chain without a link"
            );
        }
        self.chain = chain;
        if let Some(opening) = opening {
            self.push_generated(opening);
        }
        let rechained = rechain.len();
        for pending in rechain {
            let Pending {
                id,
                time,
                content,
                commit,
                pool,
                ..
            } = pending;
            let content = content.expect("only entries with content are rechained");
            self.push(Queued {
                id,
                time,
                content,
                commit,
                pool: pool.expect("entries with content hold a pool slot"),
            });
        }
        let position = self.chain.position();
        info!(
            committed = committed.len(),
            rechained,
            sequence = position.as_ref().map(|position| position.sequence),
            "ledger chain resumed after producer recovery"
        );
        Ok(committed)
    }

    fn fence(&mut self, cause: &LogError) -> Outcome {
        self.shared.set_status(LedgerStatus::Fenced);
        self.observer.status_changed(LedgerStatus::Fenced);
        let lost = self.abandon(CommitFailure::Fenced);
        error!(
            error = %cause,
            lost,
            "ledger producer fenced by a newer producer with the same transactional id; the uncommitted entries are not written"
        );
        Outcome::Halted
    }

    fn abandon(&mut self, failure: CommitFailure) -> usize {
        self.shared.wait_for_senders();
        let mut lost = 0usize;
        let mut abandoned: Vec<(String, Option<CommitSender>, Option<Pool>)> =
            std::mem::take(&mut self.batch)
                .into_iter()
                .filter(|pending| pending.content.is_some())
                .map(|pending| (pending.id, pending.commit, pending.pool))
                .collect();
        while let Ok(message) = self.receiver.try_recv() {
            if let Message::Entry(queued) = message {
                abandoned.push((queued.id, queued.commit, Some(queued.pool)));
            }
        }
        for (id, commit, pool) in abandoned {
            lost += 1;
            error!(entry_id = %id, "ledger entry not written");
            if let Some(commit) = commit
                && commit.send(Err(failure.clone())).is_err()
            {
                warn!(entry_id = %id, "the caller of an abandoned write-ahead entry had stopped waiting");
            }
            if let Some(pool) = pool {
                self.shared.settle(pool);
            }
        }
        lost
    }
}

fn backoff(attempt: u32) {
    let ceiling = BACKOFF_BASE
        .saturating_mul(1u32 << attempt.min(16))
        .min(BACKOFF_CAP);
    let mut bytes = [0u8; 8];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        warn!("the system random source failed; backing off for the full ceiling");
        std::thread::sleep(ceiling);
        return;
    }
    let fraction = u64::from_le_bytes(bytes) as f64 / u64::MAX as f64;
    std::thread::sleep(ceiling.mul_f64(fraction));
}
