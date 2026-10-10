use crate::admission::IntendedProcesses;
use crate::audit::{AuditEntry, AuditRefused, AuditReservation, AuditSink, AuditSlot};
use crate::node::{NodeLink, SessionIdentity};
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use futures::channel::oneshot;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub const TREE_SCHEMAS: &str = env!("NIGHTFALL_TREE_SCHEMAS");

struct Ledger {
    entries: RefCell<Vec<AuditEntry>>,
    capacity: Cell<usize>,
    reserved: Cell<usize>,
    hold_commits: Cell<bool>,
    commit_delay: Cell<Duration>,
    held: RefCell<Vec<oneshot::Sender<()>>>,
}

struct MemorySlot {
    ledger: Rc<Ledger>,
}

impl Drop for MemorySlot {
    fn drop(&mut self) {
        self.ledger.reserved.set(self.ledger.reserved.get() - 1);
    }
}

impl AuditSlot for MemorySlot {
    fn record(self: Box<Self>, entry: AuditEntry) -> Option<Promise<(), capnp::Error>> {
        let write_ahead = entry.write_ahead;
        self.ledger.entries.borrow_mut().push(entry);
        if !write_ahead {
            return None;
        }
        if !self.ledger.hold_commits.get() {
            let delay = self.ledger.commit_delay.get();
            if delay.is_zero() {
                return Some(Promise::ok(()));
            }
            return Some(Promise::from_future(async move {
                tokio::time::sleep(delay).await;
                Ok(())
            }));
        }
        let (sender, receiver) = oneshot::channel();
        self.ledger.held.borrow_mut().push(sender);
        Some(Promise::from_future(async move {
            receiver.await.map_err(|_| {
                capnp::Error::failed("the test ledger dropped a held commit".to_string())
            })
        }))
    }
}

pub struct MemoryAuditSink {
    ledger: Rc<Ledger>,
}

impl MemoryAuditSink {
    pub fn new() -> Rc<MemoryAuditSink> {
        Rc::new(MemoryAuditSink {
            ledger: Rc::new(Ledger {
                entries: RefCell::new(Vec::new()),
                capacity: Cell::new(usize::MAX),
                reserved: Cell::new(0),
                hold_commits: Cell::new(false),
                commit_delay: Cell::new(Duration::ZERO),
                held: RefCell::new(Vec::new()),
            }),
        })
    }

    pub fn entries(&self) -> Vec<AuditEntry> {
        self.ledger.entries.borrow().clone()
    }

    pub fn set_capacity(&self, capacity: usize) {
        self.ledger.capacity.set(capacity);
    }

    pub fn hold_commits(&self, hold: bool) {
        self.ledger.hold_commits.set(hold);
    }

    pub fn delay_commits(&self, delay: Duration) {
        self.ledger.commit_delay.set(delay);
    }

    pub fn held_commits(&self) -> usize {
        self.ledger.held.borrow().len()
    }

    pub fn commit_held(&self) {
        let held: Vec<oneshot::Sender<()>> = self.ledger.held.borrow_mut().drain(..).collect();
        for sender in held {
            if sender.send(()).is_err() {
                tracing::debug!("a held commit's call is gone");
            }
        }
    }

    fn slots(&self, count: u32) -> AuditReservation {
        let slots = (0..count)
            .map(|_| {
                self.ledger.reserved.set(self.ledger.reserved.get() + 1);
                Box::new(MemorySlot {
                    ledger: self.ledger.clone(),
                }) as Box<dyn AuditSlot>
            })
            .collect();
        AuditReservation::new(slots)
    }
}

impl AuditSink for MemoryAuditSink {
    fn reserve(&self, slots: u32) -> Result<AuditReservation, AuditRefused> {
        if self.ledger.reserved.get() + slots as usize > self.ledger.capacity.get() {
            return Err(AuditRefused {
                reason: "the test ledger is full".to_string(),
            });
        }
        Ok(self.slots(slots))
    }

    fn reserve_denial(&self) -> Result<AuditReservation, AuditRefused> {
        Ok(self.slots(1))
    }
}

pub struct TestNodeLink {
    dusk: dusk::Client,
    identity: SessionIdentity,
    epoch: Cell<u64>,
    closed: Cell<bool>,
    fresh: Cell<u32>,
    intended_processes: Option<Arc<IntendedProcesses>>,
}

impl TestNodeLink {
    pub fn new(dusk: dusk::Client, identity: SessionIdentity, epoch: u64) -> Rc<TestNodeLink> {
        Rc::new(TestNodeLink {
            dusk,
            identity,
            epoch: Cell::new(epoch),
            closed: Cell::new(false),
            fresh: Cell::new(0),
            intended_processes: None,
        })
    }

    pub fn admitting(
        dusk: dusk::Client,
        identity: SessionIdentity,
        epoch: u64,
        intended_processes: Arc<IntendedProcesses>,
    ) -> Rc<TestNodeLink> {
        Rc::new(TestNodeLink {
            dusk,
            identity,
            epoch: Cell::new(epoch),
            closed: Cell::new(false),
            fresh: Cell::new(0),
            intended_processes: Some(intended_processes),
        })
    }

    pub fn close(&self) {
        self.closed.set(true);
    }

    pub fn fresh_dusks(&self) -> u32 {
        self.fresh.get()
    }
}

impl NodeLink for TestNodeLink {
    fn epoch(&self) -> u64 {
        self.epoch.get()
    }

    fn identity(&self) -> &SessionIdentity {
        &self.identity
    }

    fn fresh_dusk(&self) -> Promise<dusk::Client, capnp::Error> {
        self.fresh.set(self.fresh.get() + 1);
        let request = self.dusk.dusk_request();
        Promise::from_future(async move { request.send().promise.await?.get()?.get_result() })
    }

    fn closed(&self) -> bool {
        self.closed.get()
    }

    fn intended_processes(&self) -> Option<&IntendedProcesses> {
        self.intended_processes.as_deref()
    }
}
