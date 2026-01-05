use dusk_program_init::InitLauncher;
use dusk_program_ps::PsLauncher;
use dusk_program_sh::ShLauncher;

fn main() {
    dusk_nix::bootstrap();
    dusk_nix::run(
        dusk_nix::StatelessLauncherSetBuilder::new(dusk_nix::LauncherSet::from_launchers(vec![
            Box::new(InitLauncher {}),
            Box::new(ShLauncher {}),
            Box::new(PsLauncher {}),
        ])),
        dusk_program_init::InitArgs::new(),
    );
}
