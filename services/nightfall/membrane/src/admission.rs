use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

pub const NO_PROCESS: u64 = 0;
pub const DEFAULT_SHELL_PID: u64 = 0xf2ef_ce60_e8c4_25d0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeKey {
    pub device: u128,
    pub installation: u128,
}

fn identifier(text: &str) -> Option<u128> {
    if text.len() != 32
        || !text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    u128::from_str_radix(text, 16).ok()
}

impl NodeKey {
    pub fn parse(device_id: &str, installation_id: &str) -> Option<NodeKey> {
        Some(NodeKey {
            device: identifier(device_id)?,
            installation: identifier(installation_id)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intent {
    pub campaign_id: Option<String>,
    pub principal: String,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntendedProcess {
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub max_commands: u32,
    pub default_shell_commands: u32,
    pub intent: Arc<Intent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    NotCaughtUp,
    ProcessWithoutIntent,
    ProcessWithoutPid,
    ProcessAfterDeadline,
    CommandBudget,
    DefaultShellWithoutIntent,
    DefaultShellBudget,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::NotCaughtUp => "not_caught_up",
            Rule::ProcessWithoutIntent => "process_without_intent",
            Rule::ProcessWithoutPid => "process_without_pid",
            Rule::ProcessAfterDeadline => "process_after_deadline",
            Rule::CommandBudget => "command_budget",
            Rule::DefaultShellWithoutIntent => "default_shell_without_intent",
            Rule::DefaultShellBudget => "default_shell_budget",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Process { pid: Option<u64>, scans_kvs: bool },
    Command { pid: u64 },
    Reap,
    Other { pid: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    Recorded,
    Removed,
    Full,
}

struct Tracked {
    intent: IntendedProcess,
    commands: u32,
}

#[derive(Default)]
struct NodeIntents {
    processes: HashMap<u64, Tracked>,
    shell_window: Option<i64>,
    shell_commands: u32,
}

#[derive(Default)]
struct Table {
    nodes: HashMap<NodeKey, NodeIntents>,
    count: usize,
}

pub struct IntendedProcesses {
    table: Mutex<Table>,
    capacity: usize,
    clock_skew_ms: i64,
    intent_wait: Duration,
    caught_up: AtomicBool,
    requested: AtomicU64,
    served: watch::Sender<u64>,
}

impl IntendedProcesses {
    pub fn new(capacity: usize, clock_skew_ms: i64, intent_wait: Duration) -> IntendedProcesses {
        IntendedProcesses {
            table: Mutex::new(Table::default()),
            capacity,
            clock_skew_ms,
            intent_wait,
            caught_up: AtomicBool::new(false),
            requested: AtomicU64::new(0),
            served: watch::Sender::new(0),
        }
    }

    pub fn intent_wait(&self) -> Duration {
        self.intent_wait
    }

    pub fn fresh(&self) -> impl Future<Output = ()> + 'static {
        let wanted = self.requested.fetch_add(1, Ordering::AcqRel) + 1;
        let mut served = self.served.subscribe();
        async move {
            if served.wait_for(|served| *served >= wanted).await.is_err() {
                tracing::debug!(
                    wanted,
                    "the table of intended processes was dropped while a call waited for it"
                );
            }
        }
    }

    pub fn requested(&self) -> u64 {
        self.requested.load(Ordering::Acquire)
    }

    pub fn serve(&self, generation: u64) {
        self.served.send_if_modified(|served| {
            let newer = generation > *served;
            if newer {
                *served = generation;
            }
            newer
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn mark_caught_up(&self) {
        self.caught_up.store(true, Ordering::Release);
    }

    pub fn caught_up(&self) -> bool {
        self.caught_up.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.lock().count
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn apply(&self, node: NodeKey, pid: u64, intent: Option<IntendedProcess>) -> Applied {
        let mut table = self.lock();
        let Some(intent) = intent else {
            let removed = table.nodes.get_mut(&node).and_then(|intents| {
                let removed = intents.processes.remove(&pid);
                let empty = intents.processes.is_empty();
                Some((removed?, empty))
            });
            if let Some((_, empty)) = removed {
                table.count -= 1;
                if empty {
                    table.nodes.remove(&node);
                }
            }
            return Applied::Removed;
        };
        let known = table
            .nodes
            .get(&node)
            .is_some_and(|intents| intents.processes.contains_key(&pid));
        if !known && table.count >= self.capacity {
            return Applied::Full;
        }
        let intents = table.nodes.entry(node).or_default();
        match intents.processes.get_mut(&pid) {
            Some(tracked) => tracked.intent = intent,
            None => {
                intents.processes.insert(
                    pid,
                    Tracked {
                        intent,
                        commands: 0,
                    },
                );
                table.count += 1;
            }
        }
        Applied::Recorded
    }

    pub fn intent_of(&self, node: NodeKey, pid: u64) -> Option<Arc<Intent>> {
        if pid == NO_PROCESS || pid == DEFAULT_SHELL_PID {
            return None;
        }
        self.lock()
            .nodes
            .get(&node)
            .and_then(|intents| intents.processes.get(&pid))
            .map(|tracked| tracked.intent.intent.clone())
    }

    pub fn evict_expired(&self, now_ms: i64) -> usize {
        let mut table = self.lock();
        let deadline = now_ms - self.clock_skew_ms;
        let mut evicted = 0;
        table.nodes.retain(|_, intents| {
            let before = intents.processes.len();
            intents
                .processes
                .retain(|_, tracked| tracked.intent.expires_at_ms >= deadline);
            evicted += before - intents.processes.len();
            !intents.processes.is_empty()
        });
        table.count -= evicted;
        evicted
    }

    fn open(&self, tracked: &Tracked, now_ms: i64) -> bool {
        tracked.intent.created_at_ms - self.clock_skew_ms <= now_ms
            && !self.expired(tracked, now_ms)
    }

    fn expired(&self, tracked: &Tracked, now_ms: i64) -> bool {
        now_ms > tracked.intent.expires_at_ms + self.clock_skew_ms
    }

    pub fn admit(&self, node: NodeKey, call: Call, now_ms: i64) -> Result<(), Rule> {
        self.judge(node, call, now_ms, true)
    }

    pub fn would_refuse(&self, node: NodeKey, call: Call, now_ms: i64) -> Option<Rule> {
        self.judge(node, call, now_ms, false).err()
    }

    fn judge(&self, node: NodeKey, call: Call, now_ms: i64, commit: bool) -> Result<(), Rule> {
        match call {
            Call::Reap => return Ok(()),
            Call::Other { pid } if pid == NO_PROCESS || pid == DEFAULT_SHELL_PID => return Ok(()),
            _ if !self.caught_up() => return Err(Rule::NotCaughtUp),
            _ => {}
        }
        let mut table = self.lock();
        let intents = table.nodes.get_mut(&node);
        match call {
            Call::Reap => Ok(()),
            Call::Process {
                pid: None,
                scans_kvs,
            } => {
                let open = intents.is_some_and(|intents| {
                    intents
                        .processes
                        .values()
                        .any(|tracked| self.open(tracked, now_ms))
                });
                if scans_kvs && open {
                    Ok(())
                } else {
                    Err(Rule::ProcessWithoutPid)
                }
            }
            Call::Process {
                pid: Some(DEFAULT_SHELL_PID),
                ..
            } => {
                let open = intents.is_some_and(|intents| {
                    intents
                        .processes
                        .values()
                        .any(|tracked| self.open(tracked, now_ms))
                });
                if open {
                    Ok(())
                } else {
                    Err(Rule::DefaultShellWithoutIntent)
                }
            }
            Call::Process { pid: Some(pid), .. } => {
                match intents.and_then(|intents| intents.processes.get(&pid)) {
                    None => Err(Rule::ProcessWithoutIntent),
                    Some(tracked) if self.expired(tracked, now_ms) => {
                        Err(Rule::ProcessAfterDeadline)
                    }
                    Some(_) => Ok(()),
                }
            }
            Call::Command {
                pid: DEFAULT_SHELL_PID,
            } => {
                let Some(intents) = intents else {
                    return Err(Rule::DefaultShellWithoutIntent);
                };
                let Some(start) = intents
                    .processes
                    .values()
                    .filter(|tracked| self.open(tracked, now_ms))
                    .map(|tracked| tracked.intent.created_at_ms)
                    .min()
                else {
                    if commit {
                        intents.shell_window = None;
                        intents.shell_commands = 0;
                    }
                    return Err(Rule::DefaultShellWithoutIntent);
                };
                let used = if intents.shell_window == Some(start) {
                    intents.shell_commands
                } else {
                    0
                };
                let allowed: u64 = intents
                    .processes
                    .values()
                    .filter(|tracked| {
                        tracked.intent.created_at_ms - self.clock_skew_ms <= now_ms
                            && tracked.intent.expires_at_ms + self.clock_skew_ms >= start
                    })
                    .map(|tracked| u64::from(tracked.intent.default_shell_commands))
                    .sum();
                if u64::from(used) >= allowed {
                    return Err(Rule::DefaultShellBudget);
                }
                if commit {
                    intents.shell_window = Some(start);
                    intents.shell_commands = used + 1;
                }
                Ok(())
            }
            Call::Command { pid: NO_PROCESS } => Err(Rule::ProcessWithoutPid),
            Call::Command { pid } => {
                match intents.and_then(|intents| intents.processes.get_mut(&pid)) {
                    None => Err(Rule::ProcessAfterDeadline),
                    Some(tracked) if self.expired(tracked, now_ms) => {
                        Err(Rule::ProcessAfterDeadline)
                    }
                    Some(tracked) if tracked.commands >= tracked.intent.max_commands => {
                        Err(Rule::CommandBudget)
                    }
                    Some(tracked) => {
                        if commit {
                            tracked.commands += 1;
                        }
                        Ok(())
                    }
                }
            }
            Call::Other { pid } => match intents.and_then(|intents| intents.processes.get(&pid)) {
                Some(tracked) if !self.expired(tracked, now_ms) => Ok(()),
                _ => Err(Rule::ProcessAfterDeadline),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKEW: i64 = 5_000;
    const NOW: i64 = 1_791_000_000_000;
    const PID: u64 = 13_792_273_858_822_192_861;

    fn node() -> NodeKey {
        NodeKey::parse(
            "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13",
            "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70",
        )
        .unwrap()
    }

    fn other_node() -> NodeKey {
        NodeKey::parse(
            "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13",
            "00000000000000000000000000000001",
        )
        .unwrap()
    }

    fn intent(max_commands: u32, default_shell_commands: u32) -> IntendedProcess {
        IntendedProcess {
            created_at_ms: NOW - 1_000,
            expires_at_ms: NOW + 60_000,
            max_commands,
            default_shell_commands,
            intent: Arc::new(Intent {
                campaign_id: Some("0192f3a4-5b6c-7d8e-9f01-23456789abcd".to_string()),
                principal: "token:0192f3a4-1111-7d8e-9f01-23456789abcd".to_string(),
                subject: "campaign:0192f3a4-5b6c-7d8e-9f01-23456789abcd".to_string(),
            }),
        }
    }

    fn table() -> IntendedProcesses {
        let table = IntendedProcesses::new(16, SKEW, Duration::ZERO);
        table.mark_caught_up();
        table
    }

    fn process(pid: u64) -> Call {
        Call::Process {
            pid: Some(pid),
            scans_kvs: false,
        }
    }

    #[test]
    fn parses_only_lowercase_identifiers_of_32_hex_digits() {
        assert!(
            NodeKey::parse(
                "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13",
                "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
            )
            .is_some()
        );
        assert!(
            NodeKey::parse(
                "3F9C0E2A7B5D4C1E8A6F0B2D9E7C5A13",
                "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
            )
            .is_none()
        );
        assert!(NodeKey::parse("3f9c", "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70").is_none());
        assert!(
            NodeKey::parse(
                "+f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13",
                "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
            )
            .is_none()
        );
    }

    #[test]
    fn refuses_every_process_call_until_caught_up() {
        let table = IntendedProcesses::new(16, SKEW, Duration::ZERO);
        table.apply(node(), PID, Some(intent(1, 3)));
        assert_eq!(
            table.admit(node(), process(PID), NOW),
            Err(Rule::NotCaughtUp)
        );
        assert_eq!(
            table.admit(node(), Call::Command { pid: PID }, NOW),
            Err(Rule::NotCaughtUp)
        );
        assert_eq!(
            table.admit(node(), Call::Other { pid: PID }, NOW),
            Err(Rule::NotCaughtUp)
        );
        assert_eq!(table.admit(node(), Call::Reap, NOW), Ok(()));
        assert_eq!(
            table.admit(node(), Call::Other { pid: NO_PROCESS }, NOW),
            Ok(())
        );
        assert_eq!(
            table.admit(
                node(),
                Call::Other {
                    pid: DEFAULT_SHELL_PID
                },
                NOW
            ),
            Ok(())
        );
        table.mark_caught_up();
        assert_eq!(table.admit(node(), process(PID), NOW), Ok(()));
    }

    #[test]
    fn admits_a_process_only_at_a_pid_intended_for_its_node() {
        let table = table();
        table.apply(node(), PID, Some(intent(1, 3)));
        assert_eq!(table.admit(node(), process(PID), NOW), Ok(()));
        assert_eq!(
            table.admit(node(), process(PID + 1), NOW),
            Err(Rule::ProcessWithoutIntent)
        );
        assert_eq!(
            table.admit(other_node(), process(PID), NOW),
            Err(Rule::ProcessWithoutIntent)
        );
    }

    #[test]
    fn refuses_a_process_without_a_pid_but_the_kvs_scan_while_the_node_has_an_open_process() {
        let table = table();
        let scan = Call::Process {
            pid: None,
            scans_kvs: true,
        };
        let unattributed = Call::Process {
            pid: None,
            scans_kvs: false,
        };
        assert_eq!(table.admit(node(), scan, NOW), Err(Rule::ProcessWithoutPid));
        table.apply(node(), PID, Some(intent(1, 3)));
        assert_eq!(
            table.admit(node(), unattributed, NOW),
            Err(Rule::ProcessWithoutPid)
        );
        assert_eq!(table.admit(node(), scan, NOW), Ok(()));
        assert_eq!(
            table.admit(node(), scan, NOW + 60_000 + SKEW + 1),
            Err(Rule::ProcessWithoutPid)
        );
        assert_eq!(
            table.admit(other_node(), scan, NOW),
            Err(Rule::ProcessWithoutPid)
        );
    }

    #[test]
    fn refuses_every_call_under_a_pid_after_its_deadline_but_kill_and_waitpid() {
        let table = table();
        table.apply(node(), PID, Some(intent(5, 3)));
        let late = NOW + 60_000 + SKEW + 1;
        assert_eq!(
            table.admit(node(), Call::Other { pid: PID }, NOW + 60_000 + SKEW),
            Ok(())
        );
        assert_eq!(
            table.admit(node(), process(PID), late),
            Err(Rule::ProcessAfterDeadline)
        );
        assert_eq!(
            table.admit(node(), Call::Command { pid: PID }, late),
            Err(Rule::ProcessAfterDeadline)
        );
        assert_eq!(
            table.admit(node(), Call::Other { pid: PID }, late),
            Err(Rule::ProcessAfterDeadline)
        );
        assert_eq!(table.admit(node(), Call::Reap, late), Ok(()));
        table.apply(node(), PID, None);
        assert_eq!(
            table.admit(node(), Call::Other { pid: PID }, NOW),
            Err(Rule::ProcessAfterDeadline)
        );
        assert_eq!(
            table.admit(node(), Call::Command { pid: PID }, NOW),
            Err(Rule::ProcessAfterDeadline)
        );
    }

    #[test]
    fn counts_shell_commands_against_the_pid_budget_and_keeps_the_count_across_a_resend() {
        let table = table();
        table.apply(node(), PID, Some(intent(2, 3)));
        assert_eq!(table.admit(node(), Call::Command { pid: PID }, NOW), Ok(()));
        assert_eq!(table.admit(node(), Call::Command { pid: PID }, NOW), Ok(()));
        assert_eq!(
            table.admit(node(), Call::Command { pid: PID }, NOW),
            Err(Rule::CommandBudget)
        );
        table.apply(node(), PID, Some(intent(3, 6)));
        assert_eq!(table.admit(node(), Call::Command { pid: PID }, NOW), Ok(()));
        assert_eq!(
            table.admit(node(), Call::Command { pid: PID }, NOW),
            Err(Rule::CommandBudget)
        );
        assert_eq!(
            table.admit(node(), Call::Command { pid: NO_PROCESS }, NOW),
            Err(Rule::ProcessWithoutPid)
        );
    }

    #[test]
    fn holds_the_default_shell_to_the_open_processes_of_its_node_and_their_budgets() {
        let table = table();
        let shell = Call::Command {
            pid: DEFAULT_SHELL_PID,
        };
        assert_eq!(
            table.admit(node(), process(DEFAULT_SHELL_PID), NOW),
            Err(Rule::DefaultShellWithoutIntent)
        );
        assert_eq!(
            table.admit(node(), shell, NOW),
            Err(Rule::DefaultShellWithoutIntent)
        );
        table.apply(node(), PID, Some(intent(1, 2)));
        table.apply(other_node(), PID, Some(intent(1, 9)));
        assert_eq!(table.admit(node(), process(DEFAULT_SHELL_PID), NOW), Ok(()));
        assert_eq!(table.admit(node(), shell, NOW), Ok(()));
        assert_eq!(table.admit(node(), shell, NOW), Ok(()));
        assert_eq!(
            table.admit(node(), shell, NOW),
            Err(Rule::DefaultShellBudget)
        );
        table.apply(
            node(),
            PID + 1,
            Some(IntendedProcess {
                created_at_ms: NOW,
                ..intent(1, 1)
            }),
        );
        assert_eq!(table.admit(node(), shell, NOW), Ok(()));
        assert_eq!(
            table.admit(node(), shell, NOW),
            Err(Rule::DefaultShellBudget)
        );
        let later = NOW + 60_000 + SKEW + 1;
        assert_eq!(
            table.admit(node(), shell, later),
            Err(Rule::DefaultShellWithoutIntent)
        );
    }

    #[test]
    fn starts_a_new_default_shell_window_when_the_earliest_open_process_closes() {
        let table = table();
        let shell = Call::Command {
            pid: DEFAULT_SHELL_PID,
        };
        table.apply(node(), PID, Some(intent(1, 1)));
        assert_eq!(table.admit(node(), shell, NOW), Ok(()));
        assert_eq!(
            table.admit(node(), shell, NOW),
            Err(Rule::DefaultShellBudget)
        );
        let next = NOW + 120_000;
        table.apply(
            node(),
            PID + 1,
            Some(IntendedProcess {
                created_at_ms: next - 1_000,
                expires_at_ms: next + 60_000,
                ..intent(1, 1)
            }),
        );
        assert_eq!(table.admit(node(), shell, next), Ok(()));
        assert_eq!(
            table.admit(node(), shell, next),
            Err(Rule::DefaultShellBudget)
        );
    }

    #[test]
    fn names_the_intent_of_a_pid_it_holds() {
        let table = IntendedProcesses::new(16, SKEW, Duration::ZERO);
        assert_eq!(table.intent_of(node(), PID), None);
        table.apply(node(), PID, Some(intent(1, 1)));
        assert_eq!(table.intent_of(node(), PID), Some(intent(1, 1).intent));
        assert_eq!(table.intent_of(other_node(), PID), None);
        table.apply(node(), DEFAULT_SHELL_PID, Some(intent(1, 1)));
        assert_eq!(table.intent_of(node(), DEFAULT_SHELL_PID), None);
        table.apply(node(), PID, None);
        assert_eq!(table.intent_of(node(), PID), None);
    }

    #[test]
    fn judges_without_counting_until_it_admits() {
        let table = table();
        let shell = Call::Command {
            pid: DEFAULT_SHELL_PID,
        };
        table.apply(node(), PID, Some(intent(1, 1)));
        for _ in 0..3 {
            assert_eq!(
                table.would_refuse(node(), Call::Command { pid: PID }, NOW),
                None
            );
            assert_eq!(table.would_refuse(node(), shell, NOW), None);
        }
        assert_eq!(table.admit(node(), Call::Command { pid: PID }, NOW), Ok(()));
        assert_eq!(table.admit(node(), shell, NOW), Ok(()));
        assert_eq!(
            table.would_refuse(node(), Call::Command { pid: PID }, NOW),
            Some(Rule::CommandBudget)
        );
        assert_eq!(
            table.would_refuse(node(), shell, NOW),
            Some(Rule::DefaultShellBudget)
        );
        assert_eq!(
            table.would_refuse(node(), process(PID + 1), NOW),
            Some(Rule::ProcessWithoutIntent)
        );
    }

    #[test]
    fn a_wait_for_fresh_intents_ends_once_the_reader_serves_its_request() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let table = Arc::new(IntendedProcesses::new(16, SKEW, Duration::from_secs(1)));
            let first = table.fresh();
            let second = table.fresh();
            assert_eq!(table.requested(), 2);
            table.serve(1);
            tokio::time::timeout(Duration::from_secs(1), first)
                .await
                .unwrap();
            let waiting = tokio::time::timeout(Duration::from_millis(50), second).await;
            assert!(
                waiting.is_err(),
                "a request was served before the reader reached it"
            );
            let third = table.fresh();
            let reader = table.clone();
            tokio::spawn(async move { reader.serve(reader.requested()) });
            tokio::time::timeout(Duration::from_secs(1), third)
                .await
                .unwrap();
            table.serve(1);
            assert_eq!(*table.served.borrow(), 3);
        });
    }

    #[test]
    fn bounds_the_table_and_evicts_what_expired() {
        let table = IntendedProcesses::new(2, SKEW, Duration::ZERO);
        assert_eq!(
            table.apply(node(), 1, Some(intent(1, 1))),
            Applied::Recorded
        );
        assert_eq!(
            table.apply(other_node(), 1, Some(intent(1, 1))),
            Applied::Recorded
        );
        assert_eq!(table.apply(node(), 2, Some(intent(1, 1))), Applied::Full);
        assert_eq!(
            table.apply(node(), 1, Some(intent(2, 2))),
            Applied::Recorded
        );
        assert_eq!(table.len(), 2);
        assert_eq!(table.evict_expired(NOW + 60_000 + SKEW), 0);
        assert_eq!(table.evict_expired(NOW + 60_000 + SKEW + 1), 2);
        assert!(table.is_empty());
        assert_eq!(
            table.apply(node(), 2, Some(intent(1, 1))),
            Applied::Recorded
        );
        assert_eq!(table.apply(node(), 2, None), Applied::Removed);
        assert_eq!(table.apply(node(), 2, None), Applied::Removed);
        assert!(table.is_empty());
    }
}
