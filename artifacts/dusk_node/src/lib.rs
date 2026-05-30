use dusk_base::dusk_program_init::Args as InitArgs;

#[unsafe(no_mangle)]
pub extern "C" fn dusk_node_run() -> i32 {
    dusk_nix::bootstrap_logging();
    dusk_nix::run(
        dusk_nix::BasicLauncherSetBuilder::new(dusk_base::launcher_set()),
        InitArgs::new("0.0.0.0", 9090)
            .as_program_args()
            .expect("build init program_args"),
    )
}
