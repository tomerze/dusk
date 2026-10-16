use crate::node::{Node, Process, ProgramArgs, VERSION, answered};
use capnp::capability::Promise;
use capnp_rpc::pry;
use dusk_capnp::dusk_capnp::{portal, stream};
use dusk_program::value::{Record, Value};
use dusk_program_kill::kill_capnp::{self, kill_args};
use dusk_program_kvs::kvs::key_id;
use dusk_program_kvs::kvs_capnp::{self, kvs_args, kvs_portal};
use dusk_program_logs::logs_capnp;
use dusk_program_ps::ps_capnp::{self, ps_args};
use dusk_program_sh::bytecode_capnp::bytecode;
use dusk_program_sh::sh_capnp::{self, output_portal, sh_portal};
use std::rc::Rc;

pub const SCRIPT_OUTPUT: &str = "simulated";

pub enum Command {
    Ps(Option<u64>),
    Kill {
        pid: u64,
        signal: u64,
    },
    KvsGet {
        keys: Vec<u64>,
        server: Option<kvs_args::server::Client>,
    },
    Logs,
}

impl Command {
    fn of(program_args: ProgramArgs) -> capnp::Result<Option<Command>> {
        let data = program_args.get_args().get_data()?;
        Ok(match program_args.get_program_id() {
            ps_capnp::PROGRAM_ID => Some(Command::Ps(
                match data.get_as::<ps_args::data::Reader>()?.which()? {
                    ps_args::data::All(()) => None,
                    ps_args::data::Pid(pid) => Some(pid),
                },
            )),
            kill_capnp::PROGRAM_ID => {
                let data = data.get_as::<kill_args::data::Reader>()?;
                Some(Command::Kill {
                    pid: data.get_pid(),
                    signal: data.get_signal(),
                })
            }
            kvs_capnp::PROGRAM_ID => match data.get_as::<kvs_args::data::Reader>()?.which()? {
                kvs_args::data::Get(keys) => Some(Command::KvsGet {
                    keys: keys?.iter().collect(),
                    server: program_args
                        .get_args()
                        .get_server()
                        .and_then(|server| server.get_as_capability())
                        .ok(),
                }),
                _ => None,
            },
            logs_capnp::PROGRAM_ID => Some(Command::Logs),
            _ => None,
        })
    }
}

pub fn commands(script: bytecode::Reader) -> capnp::Result<Option<Vec<Command>>> {
    let mut commands = Vec::new();
    for statement in script.get_statements()? {
        let bytecode::statement::Expr(expression) = statement.which()? else {
            return Ok(None);
        };
        let bytecode::statement::expr::ProgramArgs(program_args) = expression?.which()? else {
            return Ok(None);
        };
        match Command::of(program_args?)? {
            Some(command) => commands.push(command),
            None => return Ok(None),
        }
    }
    Ok(Some(commands))
}

async fn send(output: &stream::Client, value: Value) -> capnp::Result<()> {
    let mut request = output.send_request();
    value.write_to_builder(request.get().init_value())?;
    request.send().await
}

fn ps(node: &Node, filter: Option<u64>) -> Value {
    let mut names = Vec::new();
    let mut versions = Vec::new();
    let mut program_ids = Vec::new();
    let mut pids = Vec::new();
    let mut states = Vec::new();
    for process in node.processes() {
        if filter.is_some_and(|pid| pid != process.pid) {
            continue;
        }
        names.push(Value::String(process.program.name().to_string()));
        versions.push(Value::String(VERSION.to_string()));
        program_ids.push(Value::Uint(process.program.id()));
        pids.push(Value::Uint(process.pid));
        states.push(Value::String(process.state().letters().to_string()));
    }
    Value::Record(Record::with_fields(
        ps_capnp::RESULT_TYPE_ID,
        [
            (b"Name".to_vec(), Value::List(names)),
            (b"Version".to_vec(), Value::List(versions)),
            (b"Program ID".to_vec(), Value::List(program_ids)),
            (b"PID".to_vec(), Value::List(pids)),
            (b"State".to_vec(), Value::List(states)),
        ],
    ))
}

fn spans(node: &Node) -> Vec<Value> {
    node.processes()
        .into_iter()
        .filter_map(|process| process.script_ended().map(|ended| (process.pid, ended)))
        .enumerate()
        .map(|(sequence, (pid, ended))| {
            Value::Record(Record::with_fields(
                logs_capnp::SIGNAL_TYPE_ID,
                [
                    (b"sequence".to_vec(), Value::Uint(sequence as u64)),
                    (b"severity".to_vec(), Value::String("INFO".to_string())),
                    (b"name".to_vec(), Value::String("sh_exec".to_string())),
                    (b"startTimeUnixNano".to_vec(), Value::Uint(ended)),
                    (b"endTimeUnixNano".to_vec(), Value::Uint(ended)),
                    (
                        b"attributes".to_vec(),
                        Value::Record(Record::with_fields(
                            logs_capnp::ATTRIBUTES_TYPE_ID,
                            [
                                (b"pid".to_vec(), Value::String(format!("{pid:x}"))),
                                (
                                    b"namespace_id".to_vec(),
                                    Value::String(format!("{:x}", node.namespace_id)),
                                ),
                            ],
                        )),
                    ),
                ],
            ))
        })
        .collect()
}

async fn kvs_get(
    node: &Node,
    keys: Vec<u64>,
    server: Option<kvs_args::server::Client>,
    output: &stream::Client,
) -> capnp::Result<()> {
    let found: Vec<(u64, Value)> = keys
        .into_iter()
        .filter_map(|key| node.fact(key).map(|value| (key, value)))
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    if let Some(server) = server {
        let mut request = server.transpose_request();
        {
            let mut builder = request.get();
            builder.set_output(output.clone());
            let mut values = builder.reborrow().init_values(found.len() as u32);
            for (index, (_, value)) in found.iter().enumerate() {
                value.write_to_builder(values.reborrow().get(index as u32))?;
            }
            let mut keys = builder.init_keys(found.len() as u32);
            for (index, (key, _)) in found.iter().enumerate() {
                keys.set(index as u32, *key);
            }
        }
        match request.send().promise.await {
            Ok(_) => return Ok(()),
            Err(error)
                if matches!(
                    error.kind,
                    capnp::ErrorKind::MessageContainsNullCapabilityPointer
                        | capnp::ErrorKind::Disconnected
                        | capnp::ErrorKind::PrematureEndOfFile
                ) =>
            {
                tracing::debug!(%error, "sending keys without their names");
            }
            Err(error) => return Err(error),
        }
    }
    let (keys, values): (Vec<Value>, Vec<Value>) = found
        .into_iter()
        .map(|(key, value)| (Value::Uint(key), value))
        .unzip();
    send(
        output,
        Value::Record(Record::with_fields(
            kvs_capnp::SCAN_TYPE_ID,
            [
                (b"ID".to_vec(), Value::List(keys)),
                (b"Value".to_vec(), Value::List(values)),
            ],
        )),
    )
    .await
}

async fn answer(node: &Node, commands: Vec<Command>, output: &stream::Client) -> capnp::Result<()> {
    let mut result = Ok(());
    for command in commands {
        result = match command {
            Command::Ps(filter) => send(output, ps(node, filter)).await,
            Command::Kill { pid, signal } => node.kill(pid, signal),
            Command::KvsGet { keys, server } => kvs_get(node, keys, server, output).await,
            Command::Logs => {
                let mut sent = Ok(());
                for span in spans(node) {
                    sent = send(output, span).await;
                    if sent.is_err() {
                        break;
                    }
                }
                sent
            }
        };
    }
    result
}

pub struct Shell {
    node: Rc<Node>,
    process: Rc<Process>,
}

impl Shell {
    pub fn client(node: Rc<Node>, process: Rc<Process>) -> sh_portal::Client {
        capnp_rpc::new_client(Shell { node, process })
    }
}

impl portal::Server for Shell {
    fn program_id(
        &mut self,
        _params: portal::ProgramIdParams,
        mut results: portal::ProgramIdResults,
    ) -> Promise<(), capnp::Error> {
        answered("Portal.programId");
        results.get().set_program_id(sh_capnp::PROGRAM_ID);
        Promise::ok(())
    }
}

impl output_portal::Server for Shell {}

impl sh_portal::Server for Shell {
    fn sh(
        &mut self,
        params: sh_portal::ShParams,
        _results: sh_portal::ShResults,
    ) -> Promise<(), capnp::Error> {
        answered("ShPortal.sh");
        let params = pry!(params.get());
        let commands = pry!(commands(pry!(params.get_script())));
        let output = pry!(params.get_output());
        let node = self.node.clone();
        let process = self.process.clone();
        Promise::from_future(async move {
            let result = match commands {
                Some(commands) => answer(&node, commands, &output).await,
                None => send(&output, Value::String(SCRIPT_OUTPUT.to_string())).await,
            };
            process.end_script();
            if let Err(error) = output.done_request().send().promise.await {
                tracing::debug!(%error, pid = process.pid, "couldn't close the caller's stream");
            }
            result
        })
    }

    fn functions(
        &mut self,
        _params: sh_portal::FunctionsParams,
        mut results: sh_portal::FunctionsResults,
    ) -> Promise<(), capnp::Error> {
        answered("ShPortal.functions");
        results.get().init_symbols(0);
        Promise::ok(())
    }
}

pub struct KvsScan {
    node: Rc<Node>,
}

impl KvsScan {
    pub fn client(node: Rc<Node>) -> kvs_portal::Client {
        capnp_rpc::new_client(KvsScan { node })
    }
}

impl portal::Server for KvsScan {
    fn program_id(
        &mut self,
        _params: portal::ProgramIdParams,
        mut results: portal::ProgramIdResults,
    ) -> Promise<(), capnp::Error> {
        answered("Portal.programId");
        results.get().set_program_id(kvs_capnp::PROGRAM_ID);
        Promise::ok(())
    }
}

impl output_portal::Server for KvsScan {}

impl kvs_portal::Server for KvsScan {
    fn scan(
        &mut self,
        params: kvs_portal::ScanParams,
        _results: kvs_portal::ScanResults,
    ) -> Promise<(), capnp::Error> {
        answered("KvsPortal.scan");
        let output = pry!(pry!(params.get()).get_output());
        let page = Value::List(
            self.node
                .facts()
                .into_iter()
                .map(|(name, _)| Value::Uint(key_id(name)))
                .collect(),
        );
        Promise::from_future(async move {
            send(&output, page).await?;
            output.done_request().send().promise.await?;
            Ok(())
        })
    }
}
