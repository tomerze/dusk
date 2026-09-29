use dusk_core::driver::Driver;
use dusk_program::anyhow::{Ok, Result, anyhow};
use nix::unistd::gethostname;

pub(crate) struct NixDriver;

dusk_core::dusk_driver_impl!(static ref DRIVER: NixDriver = NixDriver);

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
static NEXT_TID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
std::thread_local! {
    static TID: u64 = NEXT_TID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

impl Driver for NixDriver {
    fn hostname(&self) -> Result<String> {
        Ok(gethostname()?
            .into_string()
            .map_err(|os_str| anyhow!("failed to parse hostname `{os_str:#?}` to UTF-8"))?)
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn tid(&self) -> u64 {
        nix::unistd::gettid().as_raw() as u64
    }

    #[cfg(target_vendor = "apple")]
    fn tid(&self) -> u64 {
        let mut tid = 0;
        if unsafe { nix::libc::pthread_threadid_np(nix::libc::pthread_self(), &mut tid) } != 0 {
            return 0;
        }
        tid
    }

    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    fn tid(&self) -> u64 {
        TID.try_with(|tid| *tid).unwrap_or(0)
    }

    fn exit(&self, exit_code: i32) {
        std::panic::panic_any(crate::ExitCode(exit_code));
    }
}

