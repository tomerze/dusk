use dusk_capnp::capnp::capability::FromClientHook;
use dusk_capnp::capnp_rpc;
use dusk_program_init::{init_capnp::init_args, InitArgs, InitLauncher};
use dusk_program_ps::PsLauncher;
use dusk_program_sh::ShLauncher;
use rand::Rng;
use std::sync::{Arc, Mutex, OnceLock};

pub const LISTEN_ADDR: &str = "127.0.0.1";

pub fn gen_port() -> u16 {
    let mut rng = rand::rng();
    rng.random_range(1001..=65535)
}

#[derive(Clone)]
struct LogCapture {
    logs: Arc<Mutex<Vec<(log::Level, String)>>>,
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
            .any(|(level, _)| *level == log::Level::Error)
    }

    fn get_logs(&self) -> Vec<(log::Level, String)> {
        self.logs.lock().unwrap().clone()
    }
}

impl log::Log for LogCapture {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            self.logs.lock().unwrap().push((
                record.level(),
                format!("{} - {}", record.target(), record.args()),
            ));
        }
    }

    fn flush(&self) {}
}

pub struct DuskNixImpl {
    log_capture: LogCapture,
}

impl DuskNixImpl {
    pub fn new(address: &str, port: u16) -> Self {
        let address = address.to_string();
        let log_capture = LogCapture::new();
        let log_capture_clone = log_capture.clone();

        // Set up logging for this server instance
        let logger = Box::new(log_capture_clone);
        let max_level = log::LevelFilter::Debug;

        // Spawn server thread - it runs forever so we don't store the handle
        std::thread::spawn(move || {
            // Set logger for this thread
            log::set_boxed_logger(logger).ok();
            log::set_max_level(max_level);

            dusk_nix::run(
                dusk_nix::StatelessLauncherSetBuilder::new(dusk_nix::LauncherSet::from_launchers(
                    vec![
                        Box::new(InitLauncher {}),
                        Box::new(ShLauncher {}),
                        Box::new(PsLauncher {}),
                    ],
                )),
                capnp_rpc::new_client::<init_args::Client, _>(InitArgs::new(&address, port))
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
                if level == log::Level::Error {
                    error_msg.push_str(&format!("[{:5}] {}\n", level, msg));
                }
            }
            panic!("{}", error_msg);
        }
    }
}

impl Drop for DuskNixImpl {
    fn drop(&mut self) {
        // Check for errors when the server is dropped (at end of test)
        self.assert_no_errors();
    }
}

static DUSK_CLI_BIN: OnceLock<std::path::PathBuf> = OnceLock::new();

pub fn get_dusk_cli_bin() -> &'static std::path::Path {
    DUSK_CLI_BIN.get_or_init(|| {
        // Build once and cache the path. Escargot will only rebuild if needed.
        // The key insight: use the cargo target directory which is shared across
        // all test processes. Escargot will use cargo's lock file to ensure only
        // one build happens at a time.
        escargot::CargoBuild::new()
            .bin("dusk")
            .manifest_path("../../artifacts/dusk_cli/Cargo.toml")
            .current_release()
            .run()
            .unwrap()
            .path()
            .to_owned()
    })
}
