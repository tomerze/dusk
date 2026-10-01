use core::ffi::c_void;
use dusk_base::dusk_program_init::Args as InitArgs;
use dusk_core::driver::DuskImplExit;
use dusk_program_sh_compiler_proc::compile_sh;

pub use dusk_core::ffi::*;

#[cfg(feature = "impl_nix")]
use dusk_nix as dusk_impl;
#[cfg(feature = "impl_std")]
use dusk_std as dusk_impl;
#[cfg(feature = "impl_windows")]
use dusk_windows as dusk_impl;

#[unsafe(no_mangle)]
fn dusk_main(handle: u64, _user: *mut c_void) -> Result<DuskImplExit, DuskMainFailed> {
    Ok(dusk_impl::run(
        handle,
        dusk_base::default_launcher_set,
        InitArgs::new(&compile_sh!(env!("DUSK_NODE_INIT_SCRIPT")))
            .and_then(|a| a.as_program_args())
            .map_err(|_| DuskMainFailed::InitArgs)?,
    ))
}
