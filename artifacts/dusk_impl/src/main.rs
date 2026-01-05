use dusk_program_ps::PsLauncher;
use dusk_program_sh::ShLauncher;

fn main() {
    dusk_nix::set_launcher_set_builder(dusk_nix::StatelessLauncherSetBuilder::new(
        dusk_nix::LauncherSet::from_launchers(vec![
            Box::new(ShLauncher {}),
            Box::new(PsLauncher {}),
        ]),
    ));
    dusk_nix::run();
}
