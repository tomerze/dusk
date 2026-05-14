use dusk_program_init::Args as InitArgs;

#[unsafe(no_mangle)]
pub extern "C" fn dusk_node_run() {
    dusk_nix::bootstrap_logging();
    let init_program_args = InitArgs::new("0.0.0.0", 9090)
        .as_program_args()
        .expect("build init program_args");
    dusk_nix::run(
        dusk_nix::BasicLauncherSetBuilder::new(dusk_nix::LauncherSet::from_launchers(vec![
            Box::new(dusk_program_init::Launcher::new()),
            Box::new(dusk_program_sh::Launcher::new()),
            Box::new(dusk_program_ps::Launcher::new()),
            Box::new(dusk_program_kill::Launcher::new()),
            Box::new(dusk_program_true::Launcher::new()),
            Box::new(dusk_program_false::Launcher::new()),
        ])),
        init_program_args,
    );
}
