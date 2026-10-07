use capnp::capability::{FromClientHook, Promise};
use capnp::message::ReaderOptions;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::{RpcSystem, twoparty};
use dusk_base::dusk_program_sh::sh_capnp::{self, sh_portal, sh_stop};
use dusk_base::dusk_program_sh::{ShArgs, ShMode};
use dusk_capnp::dusk_capnp::{created, dusk, process, stream, value};
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS};
use nightfall_membrane::audit::{AuditEntry, AuditEvent, Direction, EntryKind, ResultCode};
use nightfall_membrane::filter::filter_vat_network;
use nightfall_membrane::limits::{InstanceLimits, LimitState, Limits};
use nightfall_membrane::membrane::Membrane;
use nightfall_membrane::node::SessionIdentity;
use nightfall_membrane::permissions::Permissions;
use nightfall_membrane::schema::SchemaRegistry;
use nightfall_membrane::test_support::{MemoryAuditSink, TestNodeLink};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const EPOCH: u64 = 42;
const DEVICE: &str = "00112233445566778899aabbccddeeff";
const INSTALLATION: &str = "ffeeddccbbaa99887766554433221100";

const PERMISSIONS: &str = r#"
[[role]]
name = "tester"
allow = ["Dusk.process", "Dusk.run", "Dusk.ps", "Dusk.kill", "Dusk.hostname", "Dusk.waitpid", "Dusk.time", "Dusk.programs", "Dusk.namespaceId", "Dusk.dusk", "Process.*", "Portal.*", "OutputPortal.*", "ShPortal.*"]
deny = ["Dusk.settime"]
reverse_allow = ["Stream.*", "Created.created", "ShStop.stop"]
[[role]]
name = "dawn"
allow = ["Dusk.process", "Dusk.run", "Dusk.ps", "Dusk.kill", "Dusk.hostname", "Dusk.waitpid", "Dusk.time", "Process.*", "Portal.*"]
deny = ["Dusk.settime"]
reverse_allow = ["Stream.*"]
[[role]]
name = "overrider"
allow = ["Dusk.ps", "Dusk.hostname"]
quarantine_override = true
[[role]]
name = "quarantine"
allow = ["Dusk.hostname"]
[[role]]
name = "shipped"
allow = ["Dusk.process", "Dusk.run", "Dusk.ps", "Dusk.kill", "Dusk.hostname", "Dusk.waitpid", "Dusk.time", "Dusk.programs", "Dusk.namespaceId", "Dusk.dusk", "Process.*", "Portal.*", "OutputPortal.*", "ShPortal.*", "KvsPortal.get", "KvsPortal.exists", "KvsPortal.scan", "LogsPortal.*", "CpPortal.*", "SignalBatch.Ack.ack"]
deny = ["Dusk.settime", "Dusk.fleetToken"]
reverse_allow = ["Stream.*", "Created.created", "Sink.*", "LogsArgs.Server.openStream", "LogsArgs.Stream.*", "CpArgs.Server.write", "CpArgs.Server.stat", "ShStop.stop"]
[[principal]]
name = "tester-*"
roles = ["tester"]
[[principal]]
name = "shipped-*"
roles = ["shipped"]
[[principal]]
name = "overrider-*"
roles = ["overrider"]
[[principal]]
name = "dawn-*"
roles = ["dawn"]
"#;

struct Harness {
    node: Rc<TestNodeLink>,
    membrane: Rc<Membrane>,
    audit: Rc<MemoryAuditSink>,
    instance: std::sync::Arc<InstanceLimits>,
    dusk: dusk::Client,
    bootstrap: capnp::capability::Client,
}

impl Harness {
    fn entries(&self, action: &str) -> Vec<AuditEntry> {
        self.audit
            .entries()
            .into_iter()
            .filter(|entry| entry.action.as_deref() == Some(action))
            .collect()
    }

    fn events(&self, event: AuditEvent) -> Vec<AuditEntry> {
        self.audit
            .entries()
            .into_iter()
            .filter(|entry| entry.event == Some(event))
            .collect()
    }
}

fn reader_options() -> ReaderOptions {
    ReaderOptions {
        traversal_limit_in_words: Some(4 * 1024 * 1024 / 8),
        nesting_limit: 64,
    }
}

async fn connect_node(port: u16) -> dusk::Client {
    let stream = tokio::net::TcpStream::connect((LISTEN_ADDRESS, port))
        .await
        .unwrap();
    stream.set_nodelay(true).unwrap();
    let (reader, writer) = stream.into_split();
    let network = filter_vat_network(
        twoparty::VatNetwork::new(
            reader.compat(),
            writer.compat_write(),
            Side::Client,
            reader_options(),
        ),
        |kind: &str| panic!("the node sent a rejected rpc message: {kind}"),
    );
    let mut system = RpcSystem::new(Box::new(network), None);
    let dusk: dusk::Client = system.bootstrap(Side::Server);
    tokio::task::spawn_local(system);
    dusk
}

async fn membrane_harness(
    principal: &str,
    quarantined: bool,
    limits: Limits,
    port: u16,
) -> Harness {
    let node_dusk = connect_node(port).await;
    let namespace_id = node_dusk
        .namespace_id_request()
        .send()
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result();
    let identity = SessionIdentity {
        device_id: DEVICE.to_string(),
        installation_id: INSTALLATION.to_string(),
        namespace_id,
        instance: "nightfall-test".to_string(),
        quarantined,
    };
    let node = TestNodeLink::new(node_dusk, identity, EPOCH);
    let policy = Permissions::from_toml(PERMISSIONS)
        .unwrap()
        .policy_for(principal, quarantined)
        .unwrap();
    let registry =
        SchemaRegistry::load_directory(Path::new(env!("NIGHTFALL_TREE_SCHEMAS"))).unwrap();
    let bundle = registry.bundle_for(&[]);
    let audit = MemoryAuditSink::new();
    let instance = InstanceLimits::new(&limits);
    let limit_state = LimitState::new(limits, instance.clone());
    let membrane = Membrane::new(
        node.clone(),
        principal.to_string(),
        policy,
        bundle,
        audit.clone(),
        Rc::from(&b"membrane test param key"[..]),
        limit_state,
    );

    let (client_side, server_side) = tokio::io::duplex(1 << 20);
    let (server_reader, server_writer) = tokio::io::split(server_side);
    let server_network = filter_vat_network(
        twoparty::VatNetwork::new(
            server_reader.compat(),
            server_writer.compat_write(),
            Side::Server,
            reader_options(),
        ),
        |kind: &str| panic!("the client sent a rejected rpc message: {kind}"),
    );
    tokio::task::spawn_local(RpcSystem::new(
        Box::new(server_network),
        Some(membrane.bootstrap()),
    ));
    let (client_reader, client_writer) = tokio::io::split(client_side);
    let client_network = twoparty::VatNetwork::new(
        client_reader.compat(),
        client_writer.compat_write(),
        Side::Client,
        ReaderOptions::new(),
    );
    let mut client_system = RpcSystem::new(Box::new(client_network), None);
    let bootstrap: capnp::capability::Client = client_system.bootstrap(Side::Server);
    tokio::task::spawn_local(client_system);
    Harness {
        node,
        membrane,
        audit,
        instance,
        dusk: FromClientHook::new(bootstrap.hook.add_ref()),
        bootstrap,
    }
}

fn run<F: Future<Output = ()>>(test: impl FnOnce(u16) -> F) {
    let port = std::net::TcpListener::bind((LISTEN_ADDRESS, 0))
        .and_then(|listener| listener.local_addr())
        .unwrap()
        .port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&runtime, test(port));
    drop(node);
}

async fn ps(dusk: &dusk::Client) -> capnp::capability::Response<dusk::ps_results::Owned> {
    dusk.ps_request().send().promise.await.unwrap()
}

async fn sleeping_process(dusk: &dusk::Client) -> process::Client {
    let program_args = dusk_base::dusk_program_sleep::Args::duration_ms(600_000)
        .as_program_args()
        .unwrap();
    let mut request = dusk.process_request();
    program_args
        .with_reader(|reader| request.get().set_program_args(reader))
        .unwrap();
    request
        .send()
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result()
        .unwrap()
}

fn shell_server_request(
    dusk: &dusk::Client,
) -> capnp::capability::RemotePromise<dusk::process_results::Owned> {
    let program_args = ShArgs::new(ShMode::Server)
        .unwrap()
        .as_program_args()
        .unwrap();
    let mut request = dusk.process_request();
    program_args
        .with_reader(|reader| request.get().set_program_args(reader))
        .unwrap();
    request.send()
}

async fn shell_server(dusk: &dusk::Client) -> process::Client {
    shell_server_request(dusk)
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result()
        .unwrap()
}

async fn kill(dusk: &dusk::Client, pid: u64) {
    let mut request = dusk.kill_request();
    request.get().set_pid(pid);
    request.get().set_signal(15);
    request.send().promise.await.unwrap();
}

async fn pid(process: &process::Client) -> Result<u64, capnp::Error> {
    Ok(process
        .pid_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result())
}

fn is_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[test]
fn ps_records_a_call_and_a_result_and_mints_no_new_cap_ids_on_repeat() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        let first = ps(&harness.dusk).await;
        let first_count = first.get().unwrap().get_process_entries().unwrap().len();
        assert!(first_count > 0);
        let live = harness.instance.live_caps();
        assert_eq!(live, u64::from(first_count));
        let second = ps(&harness.dusk).await;
        assert_eq!(harness.instance.live_caps(), live);
        assert_eq!(
            second.get().unwrap().get_process_entries().unwrap().len(),
            first_count
        );
        let entries = harness.entries("Dusk.ps");
        let calls: Vec<&AuditEntry> = entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Call)
            .collect();
        let results: Vec<&AuditEntry> = entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Result)
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(results.len(), 2);
        for call in &calls {
            assert_eq!(call.cap_id, Some(0));
            assert_eq!(call.parent_cap_id, None);
            assert_eq!(call.direction, Some(Direction::ClientToNode));
            assert_eq!(call.device_id.as_deref(), Some(DEVICE));
            assert_eq!(call.epoch, Some(EPOCH));
            assert_eq!(call.principal, "tester-0");
            assert_eq!(call.pid, 0);
            assert!(call.write_ahead);
            assert!(is_hash(&call.param_hash));
            assert_eq!(call.result_code, None);
        }
        assert_eq!(calls[0].param_hash, calls[1].param_hash);
        for (call, result) in calls.iter().zip(&results) {
            assert_eq!(result.call_id, call.call_id);
            assert_eq!(result.result_code, Some(ResultCode::Ok));
            assert_eq!(result.param_hash, "");
            assert!(!result.write_ahead);
        }
        assert_eq!(results[0].result_cap_ids.len(), first_count as usize);
        assert!(results[0].result_cap_ids.iter().all(|cap_id| *cap_id > 0));
        assert_eq!(results[0].result_cap_ids, results[1].result_cap_ids);
        let provenance = harness.membrane.provenance();
        for cap_id in &results[0].result_cap_ids {
            let entry = provenance
                .iter()
                .find(|entry| entry.cap_id == *cap_id)
                .unwrap();
            assert_eq!(entry.parent_cap_id, Some(0));
            assert_eq!(entry.action, "Dusk.ps");
            assert_eq!(
                entry.interface_id,
                <process::Client as capnp::traits::HasTypeId>::TYPE_ID
            );
        }
        assert_eq!(harness.node.fresh_dusks(), 1);
    });
}

#[test]
fn a_process_passed_back_to_the_node_is_its_own_and_run_then_portal_succeeds() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        let process = shell_server(&harness.dusk).await;
        let mut run_request = harness.dusk.run_request();
        run_request.get().set_process(process.clone());
        run_request.send().promise.await.unwrap();
        let portal = process
            .portal_request()
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let program_id = portal
            .program_id_request()
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_program_id();
        assert_eq!(program_id, sh_capnp::PROGRAM_ID);
        let process_cap = harness.entries("Dusk.process")[1].result_cap_ids[0];
        let run_call = &harness.entries("Dusk.run")[0];
        assert_eq!(run_call.param_cap_ids, [process_cap]);
        assert_eq!(
            harness.entries("Dusk.run")[1].result_code,
            Some(ResultCode::Ok)
        );
        let portal_call = &harness.entries("Process.portal")[0];
        assert_eq!(portal_call.cap_id, Some(process_cap));
        assert_eq!(portal_call.parent_cap_id, Some(0));
        let pid = pid(&process).await.unwrap();
        kill(&harness.dusk, pid).await;
    });
}

struct Collect {
    values: Rc<RefCell<Vec<String>>>,
    finished: Rc<Cell<bool>>,
    delay: Duration,
    active: Rc<Cell<u32>>,
    peak: Rc<Cell<u32>>,
}

impl stream::Server for Collect {
    fn send(&mut self, params: stream::SendParams) -> Promise<(), capnp::Error> {
        let text = match params
            .get()
            .and_then(|params| params.get_value())
            .and_then(|value| value.which().map_err(Into::into))
        {
            Ok(value::String(text)) | Ok(value::Text(text)) => {
                text.and_then(|text| text.to_string().map_err(Into::into))
            }
            Ok(_) => Ok(String::from("<other>")),
            Err(error) => Err(error),
        };
        let text = match text {
            Ok(text) => text,
            Err(error) => return Promise::err(error),
        };
        self.values.borrow_mut().push(text);
        let active = self.active.clone();
        let peak = self.peak.clone();
        active.set(active.get() + 1);
        peak.set(peak.get().max(active.get()));
        let delay = self.delay;
        Promise::from_future(async move {
            tokio::time::sleep(delay).await;
            active.set(active.get() - 1);
            Ok(())
        })
    }

    fn done(
        &mut self,
        _params: stream::DoneParams,
        _results: stream::DoneResults,
    ) -> Promise<(), capnp::Error> {
        self.finished.set(true);
        Promise::ok(())
    }
}

struct NeverStop;

impl sh_stop::Server for NeverStop {
    fn stop(
        &mut self,
        _params: sh_stop::StopParams,
        _results: sh_stop::StopResults,
    ) -> Promise<(), capnp::Error> {
        Promise::from_future(std::future::pending())
    }
}

struct ScriptRun {
    values: Vec<String>,
    finished: bool,
    peak: u32,
}

async fn run_script(dusk: &dusk::Client, source: &str, delay: Duration) -> ScriptRun {
    dusk_base::link_anchors();
    let shell = shell_server(dusk).await;
    let mut run_request = dusk.run_request();
    run_request.get().set_process(shell.clone());
    run_request.send().promise.await.unwrap();
    let portal: sh_portal::Client = shell
        .portal_request()
        .send()
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result()
        .unwrap()
        .cast_to();
    let values = Rc::new(RefCell::new(Vec::new()));
    let finished = Rc::new(Cell::new(false));
    let peak = Rc::new(Cell::new(0));
    let output: stream::Client = capnp_rpc::new_client(Collect {
        values: values.clone(),
        finished: finished.clone(),
        delay,
        active: Rc::new(Cell::new(0)),
        peak: peak.clone(),
    });
    let mut sh_request = portal.sh_request();
    dusk_base::dusk_program_sh::client::args::compile_into(
        dusk.clone(),
        source,
        &[],
        sh_request.get().init_script(),
    )
    .await
    .unwrap();
    sh_request.get().set_output(output);
    sh_request.get().set_stop(capnp_rpc::new_client(NeverStop));
    sh_request.send().promise.await.unwrap();
    for _ in 0..200 {
        if finished.get() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    kill(dusk, pid(&shell).await.unwrap()).await;
    ScriptRun {
        values: values.borrow().clone(),
        finished: finished.get(),
        peak: peak.get(),
    }
}

#[test]
fn a_shell_script_streams_into_a_client_hosted_stream() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        let script = run_script(&harness.dusk, "echo hello", Duration::ZERO).await;
        assert_eq!(script.values, ["hello"]);
        assert!(script.finished);
        let sends = harness.entries("Stream.send");
        let send_call = sends
            .iter()
            .find(|entry| entry.kind == EntryKind::Call)
            .unwrap();
        assert_eq!(send_call.direction, Some(Direction::NodeToClient));
        assert!(!send_call.write_ahead);
        assert!(is_hash(&send_call.param_hash));
        let sh_call = harness
            .entries("ShPortal.sh")
            .into_iter()
            .find(|entry| entry.kind == EntryKind::Call)
            .unwrap();
        let output_cap = sh_call.param_cap_ids[sh_call.param_cap_ids.len() - 2];
        assert_eq!(send_call.cap_id, Some(output_cap));
        assert!(
            sends
                .iter()
                .any(|entry| entry.result_code == Some(ResultCode::Ok))
        );
        assert!(
            harness
                .entries("Stream.done")
                .iter()
                .any(|entry| entry.result_code == Some(ResultCode::Ok))
        );
    });
}

#[test]
fn a_logs_stream_through_the_shipped_dawn_role_reaches_the_client() {
    run(|port| async move {
        let harness = membrane_harness("shipped-0", false, Limits::default(), port).await;
        let path = std::env::temp_dir().join(format!(
            "nightfall-membrane-logs-{}.jsonl",
            uuid::Uuid::now_v7()
        ));
        let source = format!("logs stream file://{} --replay-only", path.display());
        run_script(&harness.dusk, &source, Duration::ZERO).await;
        let written = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::remove_file(&path).ok();
        assert!(written.lines().count() > 0);
        let open = harness.entries("LogsArgs.Server.openStream");
        assert!(
            open.iter()
                .all(|entry| entry.direction == Some(Direction::NodeToClient))
        );
        let opened = open
            .iter()
            .find(|entry| entry.kind == EntryKind::Result)
            .unwrap();
        assert_eq!(opened.result_code, Some(ResultCode::Ok));
        let stream_cap = opened.result_cap_ids[0];
        let sends = harness.entries("LogsArgs.Stream.send");
        assert!(!sends.is_empty());
        for send in &sends {
            assert_eq!(send.direction, Some(Direction::NodeToClient));
            assert_eq!(send.cap_id, Some(stream_cap));
            assert_ne!(send.result_code, Some(ResultCode::Denied));
        }
        let acks = harness.entries("SignalBatch.Ack.ack");
        assert!(!acks.is_empty());
        for ack in &acks {
            assert_eq!(ack.direction, Some(Direction::ClientToNode));
            assert_ne!(ack.result_code, Some(ResultCode::Denied));
        }
    });
}

#[test]
fn a_pipelined_result_carrying_the_bootstrap_back_to_the_node_fails() {
    run(|port| async move {
        let harness = membrane_harness("shipped-0", false, Limits::default(), port).await;
        let bootstrap = harness.bootstrap.hook.add_ref();
        let program_args = dusk_base::dusk_program_logs::client::LogsArgs::new(
            None,
            dusk_base::dusk_program_logs::FLAG_REPLAY,
            move || Ok(FromClientHook::new(bootstrap.add_ref())),
        )
        .as_program_args()
        .unwrap();
        let mut request = harness.dusk.process_request();
        program_args
            .with_reader(|reader| request.get().set_program_args(reader))
            .unwrap();
        let process = request
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let mut run_request = harness.dusk.run_request();
        run_request.get().set_process(process);
        run_request.send().promise.await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let result = loop {
            let found = harness
                .entries("LogsArgs.Server.openStream")
                .into_iter()
                .find(|entry| entry.kind == EntryKind::Result);
            if let Some(found) = found {
                break found;
            }
            assert!(std::time::Instant::now() < deadline, "no openStream result");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(result.result_code, Some(ResultCode::Failed));
        assert!(result.result_cap_ids.is_empty());
    });
}

#[test]
fn pipelined_process_portal_shares_the_cap_id_of_the_resolved_portal() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        let process_promise = shell_server_request(&harness.dusk);
        let mut run_request = harness.dusk.run_request();
        run_request
            .get()
            .set_process(process_promise.pipeline.get_result());
        let run_promise = run_request.send().promise;
        let portal_promise = process_promise
            .pipeline
            .get_result()
            .portal_request()
            .send();
        let pipelined_program_id = portal_promise
            .pipeline
            .get_result()
            .program_id_request()
            .send()
            .promise;
        run_promise.await.unwrap();
        assert_eq!(
            pipelined_program_id
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_program_id(),
            sh_capnp::PROGRAM_ID
        );
        let portal = portal_promise
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        portal.program_id_request().send().promise.await.unwrap();
        let process = process_promise
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let pid = pid(&process).await.unwrap();

        let process_result = harness
            .entries("Dusk.process")
            .into_iter()
            .find(|entry| entry.kind == EntryKind::Result)
            .unwrap();
        let process_cap = process_result.result_cap_ids[0];
        let portal_result = harness
            .entries("Process.portal")
            .into_iter()
            .find(|entry| entry.kind == EntryKind::Result)
            .unwrap();
        let portal_cap = portal_result.result_cap_ids[0];
        let program_id_calls: Vec<AuditEntry> = harness
            .entries("Portal.programId")
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::Call)
            .collect();
        assert_eq!(program_id_calls.len(), 2);
        assert!(
            program_id_calls
                .iter()
                .all(|entry| entry.cap_id == Some(portal_cap))
        );
        assert!(
            program_id_calls
                .iter()
                .all(|entry| entry.parent_cap_id == Some(process_cap))
        );
        let pid_call = harness
            .entries("Process.pid")
            .into_iter()
            .find(|entry| entry.kind == EntryKind::Call)
            .unwrap();
        assert_eq!(pid_call.cap_id, Some(process_cap));
        assert_eq!(harness.entries("Dusk.run")[0].param_cap_ids, [process_cap]);
        kill(&harness.dusk, pid).await;
    });
}

#[test]
fn a_pipelined_process_passed_to_run_reaches_the_node_as_its_own_while_the_ledger_commits() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        harness.audit.delay_commits(Duration::from_millis(50));
        let process_promise = shell_server_request(&harness.dusk);
        let mut run_request = harness.dusk.run_request();
        run_request
            .get()
            .set_process(process_promise.pipeline.get_result());
        run_request.send().promise.await.unwrap();
        let process = process_promise
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let portal = process
            .portal_request()
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let program_id = portal
            .program_id_request()
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_program_id();
        assert_eq!(program_id, sh_capnp::PROGRAM_ID);
        let process_cap = harness
            .entries("Dusk.process")
            .into_iter()
            .find(|entry| entry.kind == EntryKind::Result)
            .unwrap()
            .result_cap_ids[0];
        let run_entries = harness.entries("Dusk.run");
        assert_eq!(run_entries[0].param_cap_ids, [process_cap]);
        assert_eq!(run_entries[1].result_code, Some(ResultCode::Ok));
        let pid = pid(&process).await.unwrap();
        kill(&harness.dusk, pid).await;
    });
}

#[test]
fn denied_methods_answer_unimplemented_and_are_ledgered() {
    run(|port| async move {
        let harness = membrane_harness("dawn-0", false, Limits::default(), port).await;
        let hostname = harness
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .unwrap();
        assert!(
            !hostname
                .get()
                .unwrap()
                .get_result()
                .unwrap()
                .to_str()
                .unwrap()
                .is_empty()
        );
        let mut settime = harness.dusk.settime_request();
        settime.get().set_unix_time_ms(1);
        let error = settime.send().promise.await.err().unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Unimplemented);
        let settime_entries = harness.entries("Dusk.settime");
        assert_eq!(settime_entries.len(), 1);
        assert_eq!(settime_entries[0].kind, EntryKind::Call);
        assert_eq!(settime_entries[0].result_code, Some(ResultCode::Denied));
        assert_eq!(settime_entries[0].pid, 0);
        assert!(is_hash(&settime_entries[0].param_hash));
        let programs = harness
            .dusk
            .programs_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(programs.kind, capnp::ErrorKind::Unimplemented);
        let fresh = harness
            .dusk
            .dusk_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(fresh.kind, capnp::ErrorKind::Unimplemented);
        assert_eq!(
            harness.entries("Dusk.dusk")[0].result_code,
            Some(ResultCode::Denied)
        );
        let fleet_token = harness
            .dusk
            .fleet_token_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(fleet_token.kind, capnp::ErrorKind::Unimplemented);
        let refused = harness.entries("Dusk.fleetToken");
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].result_code, Some(ResultCode::Denied));
    });
}

#[test]
fn a_call_on_an_unknown_interface_is_rejected() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        let request = harness
            .bootstrap
            .new_call::<capnp::any_pointer::Owned, capnp::any_pointer::Owned>(
                0x1234_5678_9abc_def0,
                3,
                None,
            );
        let error = request.send().promise.await.err().unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Unimplemented);
        let entries = harness.entries("unknown:123456789abcdef0.3");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].result_code, Some(ResultCode::Denied));
        assert_eq!(entries[0].param_hash, "");
        assert_eq!(entries[0].interface_id, Some(0x1234_5678_9abc_def0));
    });
}

struct CountCreated {
    calls: Rc<Cell<u32>>,
}

impl created::Server for CountCreated {
    fn created(
        &mut self,
        _params: created::CreatedParams,
        _results: created::CreatedResults,
    ) -> Promise<(), capnp::Error> {
        self.calls.set(self.calls.get() + 1);
        Promise::ok(())
    }
}

async fn process_with_created(
    dusk: &dusk::Client,
    calls: Rc<Cell<u32>>,
) -> Result<process::Client, capnp::Error> {
    let program_args = dusk_base::dusk_program_ps::Args::new(None)
        .as_program_args()
        .unwrap();
    program_args
        .set_created(capnp_rpc::new_client(CountCreated { calls }))
        .unwrap();
    let mut request = dusk.process_request();
    program_args
        .with_reader(|reader| request.get().set_program_args(reader))
        .unwrap();
    request.send().promise.await?.get()?.get_result()
}

#[test]
fn a_node_to_client_call_outside_reverse_allow_is_denied() {
    run(|port| async move {
        let harness = membrane_harness("dawn-0", false, Limits::default(), port).await;
        let calls = Rc::new(Cell::new(0));
        let outcome = process_with_created(&harness.dusk, calls.clone()).await;
        assert!(outcome.is_err());
        assert_eq!(calls.get(), 0);
        let denied = harness.entries("Created.created");
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].direction, Some(Direction::NodeToClient));
        assert_eq!(denied[0].result_code, Some(ResultCode::Denied));

        let tester = membrane_harness("tester-1", false, Limits::default(), port).await;
        let calls = Rc::new(Cell::new(0));
        process_with_created(&tester.dusk, calls.clone())
            .await
            .unwrap();
        assert_eq!(calls.get(), 1);
        let allowed = tester.entries("Created.created");
        assert_eq!(allowed.len(), 2);
        assert_eq!(allowed[0].param_cap_ids.len(), 1);
    });
}

#[test]
fn dropping_the_membrane_breaks_derived_caps_and_in_flight_calls() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        let process = sleeping_process(&harness.dusk).await;
        let mut run_request = harness.dusk.run_request();
        run_request.get().set_process(process.clone());
        run_request.send().promise.await.unwrap();
        let pid = pid(&process).await.unwrap();
        let mut waitpid = harness.dusk.waitpid_request();
        waitpid.get().set_pid(pid);
        let in_flight = waitpid.send().promise;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let revocation = harness.membrane.revocation();
        harness.membrane.drop_membrane("killed");
        assert!(revocation.is_cancelled());
        let error = in_flight.await.err().unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Disconnected);
        assert!(error.extra.contains("session revoked"), "{}", error.extra);
        let error = process.pid_request().send().promise.await.err().unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Disconnected);
        let error = harness
            .dusk
            .ps_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Disconnected);
        let waitpid_entries = harness.entries("Dusk.waitpid");
        assert_eq!(waitpid_entries.len(), 2);
        assert_eq!(waitpid_entries[1].result_code, Some(ResultCode::Revoked));
        let dropped = harness.events(AuditEvent::MembraneDropped);
        assert_eq!(dropped.len(), 1);
        assert_eq!(
            dropped[0].event_detail.as_ref().unwrap()["reason"],
            serde_json::Value::String("killed".to_string())
        );
        assert!(harness.membrane.dropped());

        let survivor = membrane_harness("tester-1", false, Limits::default(), port).await;
        kill(&survivor.dusk, pid).await;
    });
}

#[test]
fn a_non_streaming_call_beyond_the_rate_is_rejected() {
    run(|port| async move {
        let limits = Limits {
            calls_per_second_per_principal_per_node: 2,
            ..Limits::default()
        };
        let harness = membrane_harness("tester-0", false, limits, port).await;
        harness
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .unwrap();
        harness
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .unwrap();
        let error = harness
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Overloaded);
        assert!(
            error
                .extra
                .contains("calls_per_second_per_principal_per_node")
        );
        let limited: Vec<AuditEntry> = harness
            .entries("Dusk.hostname")
            .into_iter()
            .filter(|entry| entry.result_code == Some(ResultCode::RateLimited))
            .collect();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].kind, EntryKind::Call);
        assert_eq!(limited[0].param_hash, "");
        assert!(limited[0].param_fields.is_empty());
        tokio::time::sleep(Duration::from_millis(600)).await;
        harness
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .unwrap();
    });
}

#[test]
fn streaming_calls_beyond_the_in_flight_limit_wait_instead_of_failing() {
    run(|port| async move {
        let limits = Limits {
            max_inflight_reverse_calls_per_session: 2,
            ..Limits::default()
        };
        let harness = membrane_harness("tester-0", false, limits, port).await;
        let source: String = (0..12)
            .map(|index| format!("echo value{index}\n"))
            .collect();
        let script = run_script(&harness.dusk, &source, Duration::from_millis(25)).await;
        let expected: Vec<String> = (0..12).map(|index| format!("value{index}")).collect();
        assert_eq!(script.values, expected);
        assert!(script.finished);
        assert!(
            script.peak <= 2,
            "{} streaming calls ran at once",
            script.peak
        );
        assert!(
            harness
                .audit
                .entries()
                .iter()
                .all(|entry| entry.result_code != Some(ResultCode::RateLimited))
        );
    });
}

#[test]
fn a_result_beyond_the_live_capability_bound_fails_overloaded() {
    run(|port| async move {
        let limits = Limits {
            max_live_caps_per_session: 1,
            ..Limits::default()
        };
        let harness = membrane_harness("tester-0", false, limits, port).await;
        let error = harness
            .dusk
            .ps_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Overloaded);
        assert!(error.extra.contains("max_live_caps_per_session"));
        let result = harness
            .entries("Dusk.ps")
            .into_iter()
            .find(|entry| entry.kind == EntryKind::Result)
            .unwrap();
        assert_eq!(result.result_code, Some(ResultCode::Overloaded));
        harness
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .unwrap();
    });
}

#[test]
fn a_quarantined_node_only_answers_the_quarantine_role() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", true, Limits::default(), port).await;
        harness
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .unwrap();
        let error = harness
            .dusk
            .ps_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Unimplemented);
    });
}

#[test]
fn the_ledger_refusing_a_reservation_fails_the_call_without_forwarding() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        harness.audit.set_capacity(0);
        let error = harness
            .dusk
            .ps_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Overloaded);
        assert!(harness.entries("Dusk.ps").is_empty());
        harness.audit.set_capacity(usize::MAX);
        harness.audit.hold_commits(true);
        let pending = harness.dusk.hostname_request().send().promise;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(harness.audit.held_commits(), 1);
        assert!(
            harness
                .entries("Dusk.hostname")
                .iter()
                .all(|entry| entry.kind == EntryKind::Call)
        );
        harness.audit.commit_held();
        pending.await.unwrap();
        assert_eq!(harness.entries("Dusk.hostname").len(), 2);
    });
}

#[test]
fn a_quarantine_override_passes_and_is_ledgered_with_the_call_id() {
    run(|port| async move {
        let harness = membrane_harness("overrider-0", true, Limits::default(), port).await;
        ps(&harness.dusk).await;
        let call = harness
            .entries("Dusk.ps")
            .into_iter()
            .find(|entry| entry.kind == EntryKind::Call)
            .unwrap();
        let overrides = harness.events(AuditEvent::QuarantineOverride);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].call_id, call.call_id);
        assert_eq!(overrides[0].session_id, call.session_id);
        harness
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .unwrap();
        assert_eq!(harness.events(AuditEvent::QuarantineOverride).len(), 1);
    });
}

#[test]
fn the_bootstrap_answers_only_dusk() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        let process: process::Client = FromClientHook::new(harness.bootstrap.hook.add_ref());
        let error = process.pid_request().send().promise.await.err().unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Unimplemented);
        let denied = harness.entries("Process.pid");
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].result_code, Some(ResultCode::Denied));
        assert_eq!(denied[0].cap_id, Some(0));
        assert_eq!(harness.node.fresh_dusks(), 0);
    });
}

#[test]
fn every_entry_of_a_process_at_a_fixed_pid_carries_that_pid() {
    run(|port| async move {
        dusk_base::link_anchors();
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        let fixed = 0x5eed_0000_0000_0000 | u64::from(port);
        let program_args = ShArgs::new(ShMode::Server)
            .unwrap()
            .as_program_args()
            .unwrap();
        program_args.set_pid(Some(fixed)).unwrap();
        let mut process_request = harness.dusk.process_request();
        program_args
            .with_reader(|reader| process_request.get().set_program_args(reader))
            .unwrap();
        let shell = process_request
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let mut run_request = harness.dusk.run_request();
        run_request.get().set_process(shell.clone());
        run_request.send().promise.await.unwrap();
        let portal: sh_portal::Client = shell
            .portal_request()
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap()
            .cast_to();
        let values = Rc::new(RefCell::new(Vec::new()));
        let finished = Rc::new(Cell::new(false));
        let output: stream::Client = capnp_rpc::new_client(Collect {
            values: values.clone(),
            finished: finished.clone(),
            delay: Duration::ZERO,
            active: Rc::new(Cell::new(0)),
            peak: Rc::new(Cell::new(0)),
        });
        let mut sh_request = portal.sh_request();
        dusk_base::dusk_program_sh::client::args::compile_into(
            harness.dusk.clone(),
            "echo hello",
            &[],
            sh_request.get().init_script(),
        )
        .await
        .unwrap();
        sh_request.get().set_output(output);
        sh_request.get().set_stop(capnp_rpc::new_client(NeverStop));
        sh_request.send().promise.await.unwrap();
        for _ in 0..200 {
            if finished.get() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*values.borrow(), ["hello"]);
        kill(&harness.dusk, fixed).await;

        let entries = harness.audit.entries();
        for action in [
            "Dusk.process",
            "Dusk.run",
            "Process.portal",
            "ShPortal.sh",
            "Stream.send",
            "Dusk.kill",
        ] {
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.action.as_deref() == Some(action)),
                "no {action} entry"
            );
        }
        for entry in &entries {
            assert_eq!(
                entry.pid, fixed,
                "{:?} {:?} carries pid {}",
                entry.kind, entry.action, entry.pid
            );
        }
        let provenance = harness.membrane.provenance();
        assert!(
            provenance
                .iter()
                .filter(|entry| entry.cap_id != 0)
                .all(|entry| entry.pid == fixed)
        );
    });
}

#[test]
fn a_pipelined_result_at_the_live_capability_bound_keeps_its_wrapper() {
    run(|port| async move {
        let limits = Limits {
            max_live_caps_per_session: 2,
            ..Limits::default()
        };
        let harness = membrane_harness("tester-0", false, limits, port).await;
        let process = sleeping_process(&harness.dusk).await;
        let pid = pid(&process).await.unwrap();
        assert!(pid > 0);
        let results: Vec<AuditEntry> = harness
            .entries("Dusk.process")
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::Result)
            .collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result_code, Some(ResultCode::Ok));
        let process_cap = results[0].result_cap_ids[0];
        let pid_call = harness
            .entries("Process.pid")
            .into_iter()
            .find(|entry| entry.kind == EntryKind::Call)
            .unwrap();
        assert_eq!(pid_call.cap_id, Some(process_cap));
        assert_eq!(
            harness
                .membrane
                .provenance()
                .iter()
                .filter(|entry| entry.cap_id != 0)
                .count(),
            2
        );
    });
}

#[test]
fn a_call_pipelined_on_a_call_its_caller_let_go_of_still_reaches_the_node() {
    run(|port| async move {
        let harness = membrane_harness("tester-0", false, Limits::default(), port).await;
        harness.audit.delay_commits(Duration::from_millis(200));
        let process_promise = shell_server_request(&harness.dusk);
        let program_id = process_promise
            .pipeline
            .get_result()
            .portal_request()
            .send()
            .pipeline
            .get_result()
            .program_id_request()
            .send()
            .promise;
        let process = process_promise
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let mut run_request = harness.dusk.run_request();
        run_request.get().set_process(process.clone());
        run_request.send().promise.await.unwrap();
        assert_eq!(
            program_id.await.unwrap().get().unwrap().get_program_id(),
            sh_capnp::PROGRAM_ID
        );
        assert!(
            harness
                .entries("Process.portal")
                .iter()
                .any(|entry| entry.result_code == Some(ResultCode::Ok))
        );
        kill(&harness.dusk, pid(&process).await.unwrap()).await;
    });
}
