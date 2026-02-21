use dusk_capnp::capnp::capability::FromClientHook;
use dusk_capnp::capnp_rpc;
use dusk_program_init::init_capnp::init_args;
use rand::Rng;
use std::sync::{Arc, Mutex};
use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

pub const LISTEN_ADDR: &str = "127.0.0.1";

pub fn gen_port() -> u16 {
    let mut rng = rand::rng();
    rng.random_range(1001..=65535)
}

#[derive(Clone)]
struct LogCapture {
    logs: Arc<Mutex<Vec<(Level, String)>>>,
}

impl LogCapture {
    fn new() -> Self {
        Self {
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn has_errors(&self) -> bool {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .any(|(level, _)| *level == Level::ERROR)
    }

    fn get_logs(&self) -> Vec<(Level, String)> {
        self.logs.lock().unwrap().clone()
    }
}

impl<S: tracing::Subscriber> Layer<S> for LogCapture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = *event.metadata().level();
        let target = event.metadata().target();
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.logs
            .lock()
            .unwrap()
            .push((level, format!("{} - {}", target, visitor.0)));
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }
}

pub struct DuskNixImpl {
    log_capture: LogCapture,
}

impl DuskNixImpl {
    pub fn new(address: &str, port: u16) -> Self {
        let address = address.to_string();
        let log_capture = LogCapture::new();
        let log_capture_clone = log_capture.clone();

        std::thread::spawn(move || {
            let subscriber = tracing_subscriber::registry().with(log_capture_clone);
            let _ = tracing::subscriber::set_global_default(subscriber);

            dusk_nix::run(
                dusk_nix::StatelessLauncherSetBuilder::new(dusk_nix::LauncherSet::from_launchers(
                    vec![
                        Box::new(dusk_program_init::Launcher {}),
                        Box::new(dusk_program_sh::Launcher {}),
                        Box::new(dusk_program_ps::Launcher {}),
                    ],
                )),
                capnp_rpc::new_client::<init_args::Client, _>(dusk_program_init::Args::new(
                    &address, port,
                ))
                .cast_to::<dusk_capnp::dusk_capnp::program_args::Client>(),
            );
        });

        Self { log_capture }
    }

    /// Check if any errors were logged and panic if so
    pub fn assert_no_errors(&self) {
        if self.log_capture.has_errors() {
            let logs = self.log_capture.get_logs();
            let mut error_msg = String::from("errors where logged by dusk: \n");
            for (level, msg) in logs {
                if level == Level::ERROR {
                    error_msg.push_str(&format!("[{:5}] {}\n", level, msg));
                }
            }
            panic!("{}", error_msg);
        }
    }
}

impl Drop for DuskNixImpl {
    fn drop(&mut self) {
        self.assert_no_errors();
    }
}
