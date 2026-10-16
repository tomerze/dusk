use crate::identity;
use crate::node::{Bootstrap, IMPL, Node, VERSION};
use crate::shell::SCRIPT_OUTPUT;
use capnp::capability::Promise;
use capnp::message::ReaderOptions;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::{RpcSystem, twoparty};
use dusk_capnp::dusk_capnp::{dusk, stream};
use dusk_program::stream::{Stream, StreamMixin};
use dusk_program::value::Value;
use dusk_program_sh::client::shell::Shell;
use dusk_program_sh::sh_capnp::DEFAULT_PID;
use serde_json::{Map, Value as Json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use tokio::sync::{Notify, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const SCRIPT_LOGS: &str = "logs dump --replay-only -l info";
const WORK_PID: u64 = 0x1234_5678_9abc_def0;

fn link() -> (Rc<Node>, dusk::Client) {
    let node = Node::new(
        0x0123_4567_89ab_cdef,
        identity::device_id(0, 1),
        "simulated-0-1".to_string(),
    );
    let (client_end, node_end) = tokio::io::duplex(1 << 20);
    let (reader, writer) = tokio::io::split(node_end);
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        Side::Server,
        ReaderOptions::new(),
    );
    tokio::task::spawn_local(RpcSystem::new(
        Box::new(network),
        Some(Bootstrap::client(node.clone()).client),
    ));
    let (reader, writer) = tokio::io::split(client_end);
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        Side::Client,
        ReaderOptions::new(),
    );
    let mut system = RpcSystem::new(Box::new(network), None);
    let dusk: dusk::Client = system.bootstrap(Side::Server);
    tokio::task::spawn_local(system);
    (node, dusk)
}

async fn attach(dusk: &dusk::Client, pid: u64) -> Shell {
    dusk_base::link_anchors();
    let process = Shell::recreate_sh_process(dusk.clone(), pid).await.unwrap();
    Shell::new(dusk.clone(), process).await.unwrap()
}

struct Collect {
    values: Rc<RefCell<Vec<Json>>>,
    done: Option<oneshot::Sender<()>>,
}

impl StreamMixin for Collect {
    fn send(&mut self, value: Value) -> Promise<(), capnp::Error> {
        self.values
            .borrow_mut()
            .push(serde_json::to_value(&value).unwrap());
        Promise::ok(())
    }

    fn end(&mut self) {
        if let Some(done) = self.done.take() {
            done.send(()).unwrap();
        }
    }
}

async fn sh(shell: &mut Shell, command: &str) -> anyhow::Result<Vec<Json>> {
    let values = Rc::new(RefCell::new(Vec::new()));
    let (done, ended) = oneshot::channel();
    let output: stream::Client = capnp_rpc::new_client(Stream::new(Collect {
        values: values.clone(),
        done: Some(done),
    }));
    shell
        .sh(command, output, ended, Rc::new(Notify::new()))?
        .await?;
    Ok(values.take())
}

fn record_fields(value: &Json) -> Option<&Map<String, Json>> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let (type_id, fields) = object.iter().next()?;
    type_id.starts_with("0x").then_some(())?;
    fields.as_object()
}

fn processes(values: &[Json]) -> BTreeMap<u64, (String, String)> {
    let mut processes = BTreeMap::new();
    for fields in values.iter().filter_map(record_fields) {
        let (Some(pids), Some(names), Some(states)) = (
            fields.get("PID").and_then(Json::as_array),
            fields.get("Name").and_then(Json::as_array),
            fields.get("State").and_then(Json::as_array),
        ) else {
            continue;
        };
        for ((pid, name), state) in pids.iter().zip(names).zip(states) {
            processes.insert(
                pid.as_u64().unwrap(),
                (
                    name.as_str().unwrap().to_string(),
                    state.as_str().unwrap().to_string(),
                ),
            );
        }
    }
    processes
}

fn keys(values: &[Json]) -> BTreeMap<String, Json> {
    let mut rows = BTreeMap::new();
    for fields in values.iter().filter_map(record_fields) {
        let (Some(names), Some(found)) = (
            fields.get("Key").and_then(Json::as_array),
            fields.get("Value").and_then(Json::as_array),
        ) else {
            continue;
        };
        for (name, value) in names.iter().zip(found) {
            rows.insert(name.as_str().unwrap().to_string(), value.clone());
        }
    }
    rows
}

fn running(pid: u64, name: &str, state: &str) -> (u64, (String, String)) {
    (pid, (name.to_string(), state.to_string()))
}

async fn on_a_link(test: impl AsyncFnOnce(Rc<Node>, dusk::Client)) {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (node, dusk) = link();
            test(node, dusk).await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ps_answers_the_default_shell_dawn_attached_to() {
    on_a_link(async |_, dusk| {
        let mut default = attach(&dusk, DEFAULT_PID).await;
        let values = sh(&mut default, "ps").await.unwrap();
        assert_eq!(
            processes(&values),
            BTreeMap::from([running(DEFAULT_PID, "sh", "RR")])
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kvs_get_answers_the_facts_by_name_and_leaves_no_process_behind() {
    on_a_link(async |node, dusk| {
        let mut default = attach(&dusk, DEFAULT_PID).await;
        let values = sh(&mut default, "kvs get dusk.; ps").await.unwrap();
        let facts = keys(&values);
        assert_eq!(facts["dusk.version"], Json::from(VERSION));
        assert_eq!(facts["dusk.impl"], Json::from(IMPL));
        assert_eq!(facts["dusk.device.id"], Json::from(node.device_id.clone()));
        assert_eq!(facts["dusk.hostname"], Json::from("simulated-0-1"));
        assert_eq!(facts.len(), node.facts().len());
        assert_eq!(
            processes(&values),
            BTreeMap::from([running(DEFAULT_PID, "sh", "RR")])
        );
        let missing = sh(&mut default, "kvs get dusk.config.hash")
            .await
            .unwrap_err();
        assert!(
            format!("{missing:#}").contains("no key matches"),
            "{missing:#}"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn work_at_a_fixed_pid_runs_once_streams_one_value_and_is_reaped() {
    on_a_link(async |_, dusk| {
        let mut default = attach(&dusk, DEFAULT_PID).await;
        assert!(!processes(&sh(&mut default, "ps").await.unwrap()).contains_key(&WORK_PID));

        let mut work = attach(&dusk, WORK_PID).await;
        assert_eq!(
            sh(&mut work, "echo hello; sleep 1").await.unwrap(),
            vec![Json::from(SCRIPT_OUTPUT)]
        );
        let _again = attach(&dusk, WORK_PID).await;
        assert_eq!(
            processes(&sh(&mut default, "ps").await.unwrap()),
            BTreeMap::from([
                running(WORK_PID, "sh", "RR"),
                running(DEFAULT_PID, "sh", "RR"),
            ])
        );

        let stop = format!("kill {WORK_PID:#x}; ps");
        for _ in 0..2 {
            assert_eq!(
                processes(&sh(&mut default, &stop).await.unwrap())[&WORK_PID],
                ("sh".to_string(), "Z".to_string())
            );
        }

        let logs = sh(&mut default, SCRIPT_LOGS).await.unwrap();
        let wanted = format!("{WORK_PID:x}");
        assert!(
            logs.iter().filter_map(record_fields).any(|fields| {
                fields.get("name") == Some(&Json::from("sh_exec"))
                    && fields.contains_key("endTimeUnixNano")
                    && fields
                        .get("attributes")
                        .and_then(record_fields)
                        .and_then(|attributes| attributes.get("pid"))
                        == Some(&Json::from(wanted.clone()))
            }),
            "{logs:?}"
        );

        let reaped = sh(&mut default, &format!("kill --signal 8 {WORK_PID:#x}; ps"))
            .await
            .unwrap();
        assert_eq!(
            processes(&reaped),
            BTreeMap::from([running(DEFAULT_PID, "sh", "RR")])
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn programs_lists_every_program_the_simulator_answers_for() {
    on_a_link(async |_, dusk| {
        let response = dusk.programs_request().send().promise.await.unwrap();
        let entries = response.get().unwrap().get_program_entries().unwrap();
        let listed: Vec<u64> = entries.iter().map(|entry| entry.get_program_id()).collect();
        assert_eq!(
            listed,
            crate::node::PROGRAMS
                .iter()
                .map(|(program_id, _)| *program_id)
                .collect::<Vec<_>>()
        );
        for entry in entries.iter() {
            assert_eq!(entry.get_version().unwrap().to_str().unwrap(), VERSION);
            assert!(
                !entry
                    .get_git_revision()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .is_empty()
            );
        }
    })
    .await;
}
