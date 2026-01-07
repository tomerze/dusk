use capnp::capability::FromClientHook;
use dusk_program_init::{init_capnp::init_args, InitArgs, InitLauncher};
use dusk_program_ps::PsLauncher;
use dusk_program_sh::ShLauncher;
use rand::Rng;
use std::sync::OnceLock;

pub const LISTEN_ADDR: &str = "127.0.0.1";

pub fn gen_port() -> u16 {
    let mut rng = rand::rng();
    rng.random_range(1001..=65535)
}

pub struct DuskNixImpl {}

impl DuskNixImpl {
    pub fn new(address: &str, port: u16) -> Self {
        let address = address.to_string();

        // Spawn server thread - it runs forever so we don't store the handle
        std::thread::spawn(move || {
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

        Self {}
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
