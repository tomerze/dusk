use dusk_capnp::capnp::capability::FromClientHook;
use dusk_capnp::capnp_rpc;
use dusk_program_init::init_capnp::init_args;

fn main() {
    dusk_nix::bootstrap_logging();
    dusk_nix::run(
        dusk_nix::StatelessLauncherSetBuilder::new(dusk_nix::LauncherSet::from_launchers(vec![
            Box::new(dusk_program_init::Launcher {}),
            Box::new(dusk_program_sh::Launcher {}),
            Box::new(dusk_program_ps::Launcher {}),
        ])),
        capnp_rpc::new_client::<init_args::Client, _>(dusk_program_init::Args::new(
            "0.0.0.0", 9090,
        ))
        .cast_to::<dusk_capnp::dusk_capnp::program_args::Client>(),
    );
}
