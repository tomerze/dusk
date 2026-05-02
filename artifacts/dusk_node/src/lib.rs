use dusk_capnp::capnp::capability::FromClientHook;
use dusk_capnp::capnp_rpc;
use dusk_program_init::init_capnp::init_args;

#[unsafe(no_mangle)]
pub extern "C" fn dusk_node_run() {
    dusk_nix::bootstrap_logging();
    dusk_nix::run(
        dusk_nix::BasicLauncherSetBuilder::new(dusk_nix::LauncherSet::from_launchers(vec![
            Box::new(dusk_program_init::Launcher::new()),
            Box::new(dusk_program_sh::Launcher::new()),
            Box::new(dusk_program_ps::Launcher::new()),
            Box::new(dusk_program_kill::Launcher::new()),
        ])),
        capnp_rpc::new_client::<init_args::Client, _>(dusk_program_init::Args::new(
            "0.0.0.0", 9090,
        ))
        .cast_to::<dusk_capnp::dusk_capnp::program_args::Client>(),
    );
}
