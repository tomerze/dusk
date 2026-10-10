use capnp::capability::{FromClientHook, Promise};
use dusk_base::dusk_program_sh::sh_capnp::{sh_portal, sh_stop};
use dusk_base::dusk_program_sh::{ShArgs, ShMode};
use dusk_capnp::dusk_capnp::{dusk, process, stream, value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

pub async fn ps(dusk: &dusk::Client) -> Result<u32, capnp::Error> {
    let response = dusk.ps_request().send().promise.await?;
    Ok(response.get()?.get_process_entries()?.len())
}

pub fn shell_server_request(
    dusk: &dusk::Client,
    fixed: Option<u64>,
) -> capnp::capability::RemotePromise<dusk::process_results::Owned> {
    let program_args = ShArgs::new(ShMode::Server)
        .unwrap()
        .as_program_args()
        .unwrap();
    program_args.set_pid(fixed).unwrap();
    let mut request = dusk.process_request();
    program_args
        .with_reader(|reader| request.get().set_program_args(reader))
        .unwrap();
    request.send()
}

pub async fn shell_server(dusk: &dusk::Client, fixed: Option<u64>) -> process::Client {
    shell_server_request(dusk, fixed)
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result()
        .unwrap()
}

pub async fn pid(process: &process::Client) -> Result<u64, capnp::Error> {
    Ok(process
        .pid_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result())
}

pub async fn kill(dusk: &dusk::Client, pid: u64) {
    let mut request = dusk.kill_request();
    request.get().set_pid(pid);
    request.get().set_signal(15);
    request.send().promise.await.unwrap();
    let mut reap = dusk.waitpid_request();
    reap.get().set_pid(pid);
    reap.send().promise.await.unwrap();
}

struct Collect {
    values: Rc<RefCell<Vec<String>>>,
    finished: Rc<Cell<bool>>,
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
        match text {
            Ok(text) => {
                self.values.borrow_mut().push(text);
                Promise::ok(())
            }
            Err(error) => Promise::err(error),
        }
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

pub async fn run_script(dusk: &dusk::Client, fixed: Option<u64>, source: &str) -> Vec<String> {
    dusk_base::link_anchors();
    let shell = shell_server(dusk, fixed).await;
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
        .cast_to::<sh_portal::Client>();
    let values = Rc::new(RefCell::new(Vec::new()));
    let finished = Rc::new(Cell::new(false));
    let output: stream::Client = capnp_rpc::new_client(Collect {
        values: values.clone(),
        finished: finished.clone(),
    });
    let mut request = portal.sh_request();
    dusk_base::dusk_program_sh::client::args::compile_into(
        dusk.clone(),
        source,
        &[],
        request.get().init_script(),
    )
    .await
    .unwrap();
    request.get().set_output(output);
    request.get().set_stop(capnp_rpc::new_client(NeverStop));
    request.send().promise.await.unwrap();
    for _ in 0..300 {
        if finished.get() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(finished.get(), "the script never closed its output");
    kill(dusk, pid(&shell).await.unwrap()).await;
    values.take()
}
