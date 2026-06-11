use dusk_base::dusk_program_init::Args as InitArgs;

#[unsafe(no_mangle)]
pub extern "C" fn dusk_node_run() -> i32 {
    let Ok(launcher_set) = dusk_base::default_launcher_set() else {
        return 1;
    };
    let Ok(init_args) = InitArgs::new("0.0.0.0", 9090).as_program_args() else {
        return 2;
    };
    dusk_nix::run(
        dusk_nix::BasicLauncherSetBuilder::new(launcher_set),
        init_args,
    )
}
