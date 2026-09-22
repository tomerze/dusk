use dusk_base::dusk_program_init::Args as InitArgs;
use std::ffi::c_void;

#[cfg(feature = "impl_nix")]
use dusk_nix as dusk_impl;
#[cfg(feature = "impl_std")]
use dusk_std as dusk_impl;
#[cfg(feature = "impl_windows")]
use dusk_windows as dusk_impl;

/// Runs a Dusk node until it shuts down, and returns its exit code.
#[unsafe(no_mangle)]
pub extern "C" fn dusk_node_run(_user: *mut c_void) -> i32 {
    dusk_base::link_anchors();
    let Ok(launcher_set) = dusk_base::default_launcher_set() else {
        return 1;
    };
    let init_script = dusk_program_sh_proc::sh_to_bytecode!("nightfall -l 9090");
    let Ok(init_args) = InitArgs::new(&init_script).and_then(|args| Ok(args.as_program_args()?))
    else {
        return 2;
    };
    dusk_impl::run(move || Ok(launcher_set.clone()), init_args)
}
