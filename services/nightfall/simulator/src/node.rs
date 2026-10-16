use crate::shell::{KvsScan, Shell};
use capnp::any_pointer;
use capnp::capability::{FromClientHook, Promise};
use capnp_rpc::pry;
use dusk_capnp::dusk_capnp::{dusk, portal, process, program_args};
use dusk_program::signal::Signal;
use dusk_program::value::Value;
use dusk_program_kvs::kvs::key_id;
use dusk_program_kvs::kvs_capnp::{self, kvs_args};
use dusk_program_sh::sh_capnp::{self, sh_args};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const IMPL: &str = "simulated";
pub const MAX_LIVE_PROCESSES: usize = 256;
pub const MAX_EXITED_PROCESSES: usize = 4096;

pub const PROGRAMS: [(u64, &str); 5] = [
    (dusk_program_sh::PROGRAM_ID, dusk_program_sh::GIT_REV),
    (dusk_program_kvs::PROGRAM_ID, dusk_program_kvs::GIT_REV),
    (dusk_program_ps::PROGRAM_ID, dusk_program_ps::GIT_REV),
    (dusk_program_kill::PROGRAM_ID, dusk_program_kill::GIT_REV),
    (dusk_program_logs::PROGRAM_ID, dusk_program_logs::GIT_REV),
];

pub type ProgramArgs<'a> = program_args::Reader<'a, any_pointer::Owned, any_pointer::Owned>;

pub fn answered(method: &'static str) {
    metrics::counter!("nightfall_simulator_calls_total", "method" => method).increment(1);
}

pub fn unix_milliseconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    Shell,
    Kvs,
}

impl Program {
    pub fn id(self) -> u64 {
        match self {
            Program::Shell => sh_capnp::PROGRAM_ID,
            Program::Kvs => kvs_capnp::PROGRAM_ID,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Program::Shell => "sh",
            Program::Kvs => "kvs",
        }
    }

    fn of(program_args: ProgramArgs) -> capnp::Result<Program> {
        let data = program_args.get_args().get_data()?;
        match program_args.get_program_id() {
            sh_capnp::PROGRAM_ID => match data.get_as::<sh_args::data::Reader>()?.which()? {
                sh_args::data::Server(()) => Ok(Program::Shell),
                _ => Err(capnp::Error::failed(
                    "the simulator runs shell servers, not shell scripts or prompts".to_string(),
                )),
            },
            kvs_capnp::PROGRAM_ID => match data.get_as::<kvs_args::data::Reader>()?.which()? {
                kvs_args::data::Bind(()) => Ok(Program::Kvs),
                _ => Err(capnp::Error::failed(
                    "the simulator runs kvs only to scan its keys".to_string(),
                )),
            },
            other => Err(capnp::Error::failed(format!(
                "the simulator has no program {other:#x}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Suspended,
    Running,
    Exited,
}

impl State {
    pub fn letters(self) -> &'static str {
        match self {
            State::Suspended => "S",
            State::Running => "RR",
            State::Exited => "Z",
        }
    }
}

pub struct Process {
    pub pid: u64,
    pub program: Program,
    state: Cell<State>,
    script_ended_unix_nanoseconds: Cell<Option<u64>>,
    exited: Notify,
}

impl Process {
    pub fn state(&self) -> State {
        self.state.get()
    }

    pub fn script_ended(&self) -> Option<u64> {
        self.script_ended_unix_nanoseconds.get()
    }

    pub fn end_script(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        self.script_ended_unix_nanoseconds.set(Some(now));
    }

    fn exit(&self) {
        self.state.set(State::Exited);
        self.exited.notify_waiters();
    }

    async fn until_exited(&self) {
        loop {
            let notified = self.exited.notified();
            if self.state.get() == State::Exited {
                return;
            }
            notified.await;
        }
    }
}

pub struct Node {
    pub namespace_id: u64,
    pub device_id: String,
    pub hostname: String,
    processes: RefCell<BTreeMap<u64, Rc<Process>>>,
    next_pid: Cell<u64>,
    last_heard: Cell<Instant>,
}

fn missing() -> capnp::Error {
    capnp::Error::failed("couldn't find process".to_string())
}

impl Node {
    pub fn new(namespace_id: u64, device_id: String, hostname: String) -> Rc<Node> {
        Rc::new(Node {
            namespace_id,
            device_id,
            hostname,
            processes: RefCell::new(BTreeMap::new()),
            next_pid: Cell::new(1),
            last_heard: Cell::new(Instant::now()),
        })
    }

    pub fn heard(&self) {
        self.last_heard.set(Instant::now());
    }

    pub fn last_heard(&self) -> Instant {
        self.last_heard.get()
    }

    pub fn processes(&self) -> Vec<Rc<Process>> {
        self.processes.borrow().values().cloned().collect()
    }

    pub fn facts(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("dusk.version", Value::String(VERSION.to_string())),
            ("dusk.impl", Value::String(IMPL.to_string())),
            ("dusk.hostname", Value::String(self.hostname.clone())),
            (
                "dusk.target.arch",
                Value::String(std::env::consts::ARCH.to_string()),
            ),
            (
                "dusk.target.os",
                Value::String(std::env::consts::OS.to_string()),
            ),
            ("dusk.target.bits", Value::Uint(u64::from(usize::BITS))),
            ("dusk.device.id", Value::String(self.device_id.clone())),
        ]
    }

    pub fn fact(&self, key: u64) -> Option<Value> {
        self.facts()
            .into_iter()
            .find(|(name, _)| key_id(name) == key)
            .map(|(_, value)| value)
    }

    fn create(&self, program: Program, fixed: Option<u64>) -> capnp::Result<Rc<Process>> {
        let mut processes = self.processes.borrow_mut();
        let pid = match fixed {
            Some(pid) => {
                if let Some(process) = processes.get(&pid)
                    && process.state() != State::Exited
                {
                    return Ok(process.clone());
                }
                pid
            }
            None => {
                let mut pid = self.next_pid.get();
                while processes.contains_key(&pid) {
                    pid = pid.wrapping_add(1);
                }
                self.next_pid.set(pid.wrapping_add(1));
                pid
            }
        };
        let exited = processes
            .values()
            .filter(|process| process.state() == State::Exited)
            .count();
        if processes.len() - exited >= MAX_LIVE_PROCESSES {
            return Err(capnp::Error::overloaded(format!(
                "the simulated node holds {MAX_LIVE_PROCESSES} live processes already"
            )));
        }
        if exited >= MAX_EXITED_PROCESSES {
            return Err(capnp::Error::overloaded(format!(
                "the simulated node holds {MAX_EXITED_PROCESSES} exited processes nobody reaped"
            )));
        }
        let process = Rc::new(Process {
            pid,
            program,
            state: Cell::new(State::Suspended),
            script_ended_unix_nanoseconds: Cell::new(None),
            exited: Notify::new(),
        });
        processes.insert(pid, process.clone());
        Ok(process)
    }

    fn get(&self, pid: u64) -> capnp::Result<Rc<Process>> {
        self.processes
            .borrow()
            .get(&pid)
            .cloned()
            .ok_or_else(missing)
    }

    fn run(&self, pid: u64) -> capnp::Result<Rc<Process>> {
        let process = self.get(pid)?;
        if process.state() == State::Suspended {
            process.state.set(State::Running);
        }
        Ok(process)
    }

    fn remove(&self, pid: u64) {
        self.processes.borrow_mut().remove(&pid);
    }

    pub fn kill(&self, pid: u64, signal: u64) -> capnp::Result<()> {
        let process = self.get(pid)?;
        match Signal::from(signal) {
            Signal::Sweep => {
                if process.state() == State::Suspended {
                    process.exit();
                    self.remove(pid);
                }
            }
            Signal::Reap => {
                if process.state() == State::Exited {
                    self.remove(pid);
                }
            }
            _ if process.state() == State::Exited => {
                return Err(capnp::Error::failed("process has exited".to_string()));
            }
            Signal::Terminate => process.exit(),
            _ => {}
        }
        Ok(())
    }

    async fn waitpid(&self, pid: u64) -> capnp::Result<()> {
        let process = self.get(pid)?;
        process.until_exited().await;
        self.remove(pid);
        Ok(())
    }
}

pub struct Bootstrap {
    node: Rc<Node>,
}

impl Bootstrap {
    pub fn client(node: Rc<Node>) -> dusk::Client {
        capnp_rpc::new_client(Bootstrap { node })
    }
}

impl dusk::Server for Bootstrap {
    fn process(
        &mut self,
        params: dusk::ProcessParams,
        mut results: dusk::ProcessResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.process");
        let program_args = pry!(pry!(params.get()).get_program_args());
        let program = pry!(Program::of(program_args));
        let fixed = match pry!(program_args.get_pid().which()) {
            program_args::pid::Auto(()) => None,
            program_args::pid::Fixed(pid) => Some(pid),
        };
        let process = pry!(self.node.create(program, fixed));
        results
            .get()
            .set_result(ProcessServer::client(self.node.clone(), process));
        Promise::ok(())
    }

    fn run(
        &mut self,
        params: dusk::RunParams,
        _results: dusk::RunResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.run");
        let process = pry!(pry!(params.get()).get_process());
        let node = self.node.clone();
        Promise::from_future(async move {
            let pid = process
                .pid_request()
                .send()
                .promise
                .await?
                .get()?
                .get_result();
            node.run(pid).map(|_| ())
        })
    }

    fn ps(
        &mut self,
        _params: dusk::PsParams,
        mut results: dusk::PsResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.ps");
        let processes = self.node.processes();
        let mut entries = results.get().init_process_entries(processes.len() as u32);
        for (index, process) in processes.into_iter().enumerate() {
            let mut entry = entries.reborrow().get(index as u32);
            entry.set_pid(process.pid);
            entry.set_ready(process.state() == State::Running);
            entry.set_suspended(process.state() == State::Suspended);
            entry.set_exited(process.state() == State::Exited);
            entry.set_process(ProcessServer::client(self.node.clone(), process));
        }
        Promise::ok(())
    }

    fn kill(
        &mut self,
        params: dusk::KillParams,
        _results: dusk::KillResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.kill");
        let params = pry!(params.get());
        pry!(self.node.kill(params.get_pid(), params.get_signal()));
        Promise::ok(())
    }

    fn hostname(
        &mut self,
        _params: dusk::HostnameParams,
        mut results: dusk::HostnameResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.hostname");
        results.get().set_result(self.node.hostname.as_str());
        Promise::ok(())
    }

    fn waitpid(
        &mut self,
        params: dusk::WaitpidParams,
        _results: dusk::WaitpidResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.waitpid");
        let pid = pry!(params.get()).get_pid();
        let node = self.node.clone();
        Promise::from_future(async move { node.waitpid(pid).await })
    }

    fn time(
        &mut self,
        _params: dusk::TimeParams,
        mut results: dusk::TimeResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.time");
        self.node.heard();
        results.get().set_unix_time_ms(unix_milliseconds());
        Promise::ok(())
    }

    fn programs(
        &mut self,
        _params: dusk::ProgramsParams,
        mut results: dusk::ProgramsResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.programs");
        let mut entries = results.get().init_program_entries(PROGRAMS.len() as u32);
        for (index, (program_id, git_revision)) in PROGRAMS.iter().enumerate() {
            let mut entry = entries.reborrow().get(index as u32);
            entry.set_program_id(*program_id);
            entry.set_version(VERSION);
            entry.set_git_revision(*git_revision);
        }
        Promise::ok(())
    }

    fn namespace_id(
        &mut self,
        _params: dusk::NamespaceIdParams,
        mut results: dusk::NamespaceIdResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.namespaceId");
        results.get().set_result(self.node.namespace_id);
        Promise::ok(())
    }

    fn dusk(
        &mut self,
        _params: dusk::DuskParams,
        mut results: dusk::DuskResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.dusk");
        results
            .get()
            .set_result(Bootstrap::client(self.node.clone()));
        Promise::ok(())
    }
}

pub struct ProcessServer {
    node: Rc<Node>,
    process: Rc<Process>,
}

impl ProcessServer {
    pub fn client(node: Rc<Node>, process: Rc<Process>) -> process::Client {
        capnp_rpc::new_client(ProcessServer { node, process })
    }
}

impl process::Server for ProcessServer {
    fn pid(
        &mut self,
        _params: process::PidParams,
        mut results: process::PidResults,
    ) -> Promise<(), capnp::Error> {
        answered("Process.pid");
        results.get().set_result(self.process.pid);
        Promise::ok(())
    }

    fn program_id(
        &mut self,
        _params: process::ProgramIdParams,
        mut results: process::ProgramIdResults,
    ) -> Promise<(), capnp::Error> {
        answered("Process.programId");
        results.get().set_result(self.process.program.id());
        Promise::ok(())
    }

    fn name(
        &mut self,
        _params: process::NameParams,
        mut results: process::NameResults,
    ) -> Promise<(), capnp::Error> {
        answered("Process.name");
        results.get().set_result(self.process.program.name());
        Promise::ok(())
    }

    fn version(
        &mut self,
        _params: process::VersionParams,
        mut results: process::VersionResults,
    ) -> Promise<(), capnp::Error> {
        answered("Process.version");
        results.get().set_result(VERSION);
        Promise::ok(())
    }

    fn run(
        &mut self,
        _params: process::RunParams,
        _results: process::RunResults,
    ) -> Promise<(), capnp::Error> {
        answered("Process.run");
        let process = pry!(self.node.run(self.process.pid));
        Promise::from_future(async move {
            process.until_exited().await;
            Ok(())
        })
    }

    fn portal(
        &mut self,
        _params: process::PortalParams,
        mut results: process::PortalResults,
    ) -> Promise<(), capnp::Error> {
        answered("Process.portal");
        let portal: portal::Client = match self.process.program {
            Program::Shell => Shell::client(self.node.clone(), self.process.clone()).cast_to(),
            Program::Kvs => KvsScan::client(self.node.clone()).cast_to(),
        };
        results.get().set_result(portal);
        Promise::ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exited_processes_hold_no_live_slot_but_are_bounded_on_their_own() {
        let node = Node::new(1, "device".to_string(), "host".to_string());
        let live = MAX_LIVE_PROCESSES as u64;
        for pid in 1..=live {
            node.create(Program::Shell, Some(pid)).unwrap();
        }
        let refused = node.create(Program::Shell, Some(live + 1)).err().unwrap();
        assert_eq!(refused.kind, capnp::ErrorKind::Overloaded);
        node.kill(1, 15).unwrap();
        node.create(Program::Shell, Some(live + 1)).unwrap();
        for pid in 2..=live + 1 {
            node.kill(pid, 15).unwrap();
        }
        let mut next = live + 2;
        while node.processes().len() < MAX_EXITED_PROCESSES {
            node.create(Program::Shell, Some(next)).unwrap();
            node.kill(next, 15).unwrap();
            next += 1;
        }
        let refused = node.create(Program::Shell, Some(next)).err().unwrap();
        assert_eq!(refused.kind, capnp::ErrorKind::Overloaded);
        node.kill(1, 8).unwrap();
        node.create(Program::Shell, Some(next)).unwrap();
    }
}
