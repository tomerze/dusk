use dusk_capnp::capnp::capability::FromClientHook;
use dusk_capnp::capnp_rpc;
use dusk_program_init::{InitArgs, InitLauncher, init_capnp::init_args};
use dusk_program_ps::PsLauncher;
use dusk_program_sh::ShLauncher;

fn main() {
    dusk_nix::bootstrap_logging();
    dusk_nix::run(
        dusk_nix::StatelessLauncherSetBuilder::new(dusk_nix::LauncherSet::from_launchers(vec![
            Box::new(InitLauncher {}),
            Box::new(ShLauncher {}),
            Box::new(PsLauncher {}),
        ])),
        capnp_rpc::new_client::<init_args::Client, _>(InitArgs::new("0.0.0.0", 9090))
            .cast_to::<dusk_capnp::dusk_capnp::program_args::Client>(),
    );
}
