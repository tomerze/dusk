use dusk_program_ps::PsLauncher;
use dusk_program_sh::ShLauncher;

fn main() {
    let launchers = dusk_nix::LauncherSet::new();
    launchers.add(Box::new(ShLauncher {}));
    launchers.add(Box::new(PsLauncher {}));

    dusk_nix::set_launchers(launchers);
    dusk_nix::main();
}
