use dusk_base::dusk_program_init::Args as InitArgs;
use std::ffi::{CStr, c_char, c_void};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[cfg(feature = "impl_nix")]
use dusk_nix as dusk_impl;
#[cfg(feature = "impl_std")]
use dusk_std as dusk_impl;
#[cfg(feature = "impl_windows")]
use dusk_windows as dusk_impl;

/// Where this node listens when it is handed no address of its own.
pub const DEFAULT_LISTEN_ADDRESS: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 9090);

/// Runs a Dusk node until it shuts down, and returns its exit code.
///
/// `user` is whatever the program running the node gives it at run time. This
/// template reads it as the `ip:port` to listen on, and listens on
/// [`DEFAULT_LISTEN_ADDRESS`] when it is null - a node built from this template
/// is free to decide the pointer means something else entirely.
///
/// # Safety
///
/// `user` is either null, or a pointer to a NUL-terminated string that stays
/// valid for the duration of the call.
#[unsafe(no_mangle)]
pub extern "C" fn dusk_node_run(_user: *mut c_void) -> i32 {
    let Ok(launcher_set) = dusk_base::default_launcher_set() else {
        return 1;
    };
    let init_script = dusk_program_sh_compiler_proc::compile_sh!("nightfall -l 0.0.0.0:9090");
    let Ok(init_args) = InitArgs::new(&init_script).and_then(|args| Ok(args.as_program_args()?))
    else {
        return 2;
    };
    dusk_impl::run(move || Ok(launcher_set.clone()), init_args)
}

/// # Safety
///
/// As [`dusk_node_run`].
unsafe fn listen_address(user: *const c_char) -> Option<SocketAddr> {
    if user.is_null() {
        return Some(DEFAULT_LISTEN_ADDRESS);
    }
    unsafe { CStr::from_ptr(user) }.to_str().ok()?.parse().ok()
}
