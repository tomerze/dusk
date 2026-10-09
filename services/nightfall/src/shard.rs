use crate::audit::LedgerAudit;
use crate::events::DisconnectReason;
use crate::node_state::Scope;
use crate::server::Shared;
use crate::session::NodeSession;
use nightfall_membrane::limits::LimitState;
use nightfall_membrane::permissions::Permissions;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

pub const HANDOFF_QUEUE: usize = 1024;

pub enum Control {
    Close {
        namespace_id: u64,
        epoch: u64,
        reason: DisconnectReason,
    },
    Kill {
        namespace_id: u64,
        epoch: u64,
        by: String,
    },
    NodeState,
    Recheck,
    Permissions(Arc<Permissions>),
    KillClient {
        session_id: String,
        by: String,
        reply: oneshot::Sender<bool>,
    },
    Describe {
        namespace_id: u64,
        reply: oneshot::Sender<Option<Value>>,
    },
    Provenance {
        namespace_id: u64,
        reply: oneshot::Sender<Option<Value>>,
    },
    Stop,
}

pub struct Handoff {
    pub stream: std::net::TcpStream,
    pub namespace_id: u64,
    pub remote: SocketAddr,
    pub relayed: bool,
}

pub struct ShardHandle {
    pub control: mpsc::UnboundedSender<Control>,
    pub handoff: mpsc::Sender<Handoff>,
    pub flips: Arc<Mutex<HashSet<Scope>>>,
}

pub struct ShardReceivers {
    control: mpsc::UnboundedReceiver<Control>,
    handoff: mpsc::Receiver<Handoff>,
}

impl ShardHandle {
    pub fn new() -> (ShardHandle, ShardReceivers) {
        let (control, control_receiver) = mpsc::unbounded_channel();
        let (handoff, handoff_receiver) = mpsc::channel(HANDOFF_QUEUE);
        (
            ShardHandle {
                control,
                handoff,
                flips: Arc::default(),
            },
            ShardReceivers {
                control: control_receiver,
                handoff: handoff_receiver,
            },
        )
    }
}

pub struct ShardListeners {
    pub fleet: std::net::TcpListener,
    pub provision: Option<std::net::TcpListener>,
    pub inner: std::net::TcpListener,
    pub relay: std::net::TcpListener,
}

pub struct Shard {
    pub index: usize,
    pub shared: Arc<Shared>,
    sessions: RefCell<HashMap<u64, Rc<NodeSession>>>,
    devices: RefCell<HashMap<u128, HashSet<u64>>>,
    pub audit: Rc<LedgerAudit>,
    pub limits: Rc<LimitState>,
    pub param_key: Rc<[u8]>,
}

impl Shard {
    pub fn session(&self, namespace_id: u64) -> Option<Rc<NodeSession>> {
        self.sessions.borrow().get(&namespace_id).cloned()
    }

    pub fn session_count(&self) -> usize {
        self.sessions.borrow().len()
    }

    fn unindex(&self, session: &NodeSession) {
        let mut devices = self.devices.borrow_mut();
        let device = session.identity.device;
        if let Some(namespaces) = devices.get_mut(&device) {
            namespaces.remove(&session.namespace_id);
            if namespaces.is_empty() {
                devices.remove(&device);
            }
        }
    }

    pub fn add_session(&self, session: Rc<NodeSession>) -> Option<Rc<NodeSession>> {
        let displaced = self
            .sessions
            .borrow_mut()
            .insert(session.namespace_id, session.clone());
        if let Some(displaced) = &displaced {
            self.unindex(displaced);
        }
        self.devices
            .borrow_mut()
            .entry(session.identity.device)
            .or_default()
            .insert(session.namespace_id);
        displaced
    }

    pub fn remove_session(&self, session: &Rc<NodeSession>) {
        let removed = {
            let mut sessions = self.sessions.borrow_mut();
            let current = sessions
                .get(&session.namespace_id)
                .is_some_and(|current| Rc::ptr_eq(current, session));
            current && sessions.remove(&session.namespace_id).is_some()
        };
        if removed {
            self.unindex(session);
        }
    }

    fn sessions_of(&self, scope: Scope) -> Vec<Rc<NodeSession>> {
        let device = match scope {
            Scope::Device(device) | Scope::Installation(device, _) => device,
        };
        let namespaces: Vec<u64> = self
            .devices
            .borrow()
            .get(&device)
            .map(|namespaces| namespaces.iter().copied().collect())
            .unwrap_or_default();
        namespaces
            .into_iter()
            .filter_map(|namespace_id| self.session(namespace_id))
            .filter(|session| scope.matches(session.identity))
            .collect()
    }

    pub fn update_gauge(&self) {
        metrics::gauge!("nightfall_sessions", "shard" => self.index.to_string())
            .set(self.sessions.borrow().len() as f64);
    }

    fn sessions_snapshot(&self) -> Vec<Rc<NodeSession>> {
        self.sessions.borrow().values().cloned().collect()
    }

    fn apply(self: &Rc<Self>, control: Control) -> bool {
        match control {
            Control::Close {
                namespace_id,
                epoch,
                reason,
            } => {
                if let Some(session) = self.session(namespace_id)
                    && session.epoch == epoch
                {
                    session.close(reason);
                }
            }
            Control::Kill {
                namespace_id,
                epoch,
                by,
            } => {
                if let Some(session) = self.session(namespace_id)
                    && session.epoch == epoch
                {
                    session.kill(&by);
                }
            }
            Control::NodeState => {
                let scopes: Vec<Scope> = match self.shared.shards.get(self.index) {
                    Some(handle) => handle
                        .flips
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .drain()
                        .collect(),
                    None => Vec::new(),
                };
                for scope in scopes {
                    for session in self.sessions_of(scope) {
                        session.node_state_changed();
                    }
                }
            }
            Control::Recheck => {
                for session in self.sessions_snapshot() {
                    session.node_state_changed();
                }
            }
            Control::Permissions(permissions) => {
                for session in self.sessions_snapshot() {
                    session.permissions_changed(&permissions);
                }
            }
            Control::KillClient {
                session_id,
                by,
                reply,
            } => {
                let killed = self
                    .sessions_snapshot()
                    .iter()
                    .any(|session| session.kill_client(&session_id, &by));
                if reply.send(killed).is_err() {
                    tracing::debug!("the kill request was abandoned");
                }
            }
            Control::Describe {
                namespace_id,
                reply,
            } => {
                let described = self.session(namespace_id).map(|session| session.describe());
                if reply.send(described).is_err() {
                    tracing::debug!("the describe request was abandoned");
                }
            }
            Control::Provenance {
                namespace_id,
                reply,
            } => {
                let provenance = self
                    .session(namespace_id)
                    .map(|session| session.provenance());
                if reply.send(provenance).is_err() {
                    tracing::debug!("the provenance request was abandoned");
                }
            }
            Control::Stop => {
                for session in self.sessions_snapshot() {
                    session.close(DisconnectReason::Shutdown);
                }
                return false;
            }
        }
        true
    }
}

pub fn run(
    index: usize,
    shared: Arc<Shared>,
    listeners: ShardListeners,
    receivers: ShardReceivers,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(shard = index, %error, "a shard runtime could not start");
            return;
        }
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let limits = nightfall_membrane::limits::LimitState::new(
            shared.limits.clone(),
            shared.instance_limits.clone(),
        );
        let shard = Rc::new(Shard {
            index,
            audit: Rc::new(shared.audit.clone()),
            limits,
            param_key: Rc::from(&*shared.param_key),
            sessions: RefCell::new(HashMap::new()),
            devices: RefCell::new(HashMap::new()),
            shared,
        });
        shard.update_gauge();
        if let Err(error) = crate::listener::spawn(&shard, listeners) {
            tracing::error!(
                shard = index,
                error = format!("{error:#}"),
                "a shard could not listen"
            );
            return;
        }
        tracing::info!(shard = index, "shard started");
        let ShardReceivers {
            mut control,
            mut handoff,
        } = receivers;
        loop {
            tokio::select! {
                command = control.recv() => match command {
                    Some(command) => {
                        if !shard.apply(command) {
                            break;
                        }
                    }
                    None => break,
                },
                handed = handoff.recv() => {
                    if let Some(handed) = handed {
                        crate::listener::take_handoff(&shard, handed);
                    }
                }
            }
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while shard.session_count() > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tracing::info!(shard = index, "shard stopped");
    });
}
