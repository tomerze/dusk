pub use dusk_program;
pub use dusk_program_cp;
pub use dusk_program_date;
pub use dusk_program_echo;
pub use dusk_program_false;
pub use dusk_program_hostname;
pub use dusk_program_init;
pub use dusk_program_kill;
pub use dusk_program_kvs;
pub use dusk_program_logs;
pub use dusk_program_nightfall;
pub use dusk_program_programs;
pub use dusk_program_ps;
pub use dusk_program_sh;
pub use dusk_program_sleep;
pub use dusk_program_true;

use dusk_program::launcher_set::LauncherSet;

/// LauncherSet with launchers for all programs in base
pub fn default_launcher_set() -> anyhow::Result<LauncherSet> {
    launcher_set(dusk_program_kvs::KvsConfig::default())
}

pub fn launcher_set(kvs_config: dusk_program_kvs::KvsConfig) -> anyhow::Result<LauncherSet> {
    Ok(LauncherSet::from_launchers(vec![
        Box::new(dusk_program_logs::Launcher::new(
            dusk_program_logs::LogsConfig::default(),
        )?),
        Box::new(dusk_program_cp::Launcher::new()),
        Box::new(dusk_program_date::Launcher::new()),
        Box::new(dusk_program_echo::Launcher::new()),
        Box::new(dusk_program_false::Launcher::new()),
        Box::new(dusk_program_hostname::Launcher::new()),
        Box::new(dusk_program_init::Launcher::new()),
        Box::new(dusk_program_kill::Launcher::new()),
        Box::new(dusk_program_kvs::Launcher::new(kvs_config)?),
        Box::new(dusk_program_nightfall::Launcher::new()),
        Box::new(dusk_program_programs::Launcher::new()),
        Box::new(dusk_program_ps::Launcher::new()),
        Box::new(dusk_program_sh::Launcher::new()),
        Box::new(dusk_program_sleep::Launcher::new()),
        Box::new(dusk_program_true::Launcher::new()),
    ]))
}

/// Reference every shell-entry function under `std::hint::black_box`
/// so rustc passes each program rlib to the linker. Without this
/// (or an equivalent reference at the binary's source level), rustc's
/// unused-extern elision drops the rlibs and the `#[distributed_slice]`
/// registrations vanish before reaching the linker.
#[cfg(feature = "client")]
pub fn link_anchors() {
    use std::hint::black_box;
    black_box(dusk_program_logs::client::sh_entry);
    black_box(dusk_program_cp::client::sh_entry);
    black_box(dusk_program_date::client::sh_entry);
    black_box(dusk_program_echo::client::sh_entry);
    black_box(dusk_program_false::client::sh_entry);
    black_box(dusk_program_hostname::client::sh_entry);
    black_box(dusk_program_kill::client::sh_entry);
    black_box(dusk_program_kvs::client::sh_entry);
    black_box(dusk_program_nightfall::client::sh_entry);
    black_box(dusk_program_programs::client::sh_entry);
    black_box(dusk_program_ps::client::sh_entry);
    black_box(dusk_program_sleep::client::sh_entry);
    black_box(dusk_program_true::client::sh_entry);
}
