use dusk_program_ps::PsLauncher;
use dusk_program_sh::ShLauncher;

fn main() {
    dusk_nix::set_launchers(|| {
        let launchers = dusk_nix::LauncherSet::new();
        launchers.add(Box::new(ShLauncher {}));
        launchers.add(Box::new(PsLauncher {}));
        launchers
    });
    dusk_nix::run();
}
