use crate::driver::DuskImplExit;
use core::ffi::c_void;
use dusk_program::handle::{self, HandleState, new_handle, unbind_handle};
use portable_atomic::Ordering;

pub const DUSK_RESULT_SOURCE_NAMESPACE: i64 = 0;
pub const DUSK_RESULT_SOURCE_DUSK_MAIN: i64 = 1;
pub const DUSK_RESULT_SOURCE_RUST_PANIC: i64 = 2;

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuskMainFailed {
    InitArgs = 1,
    UnknownHandle = 2,
    BoundHandle = 3,
    Spawn = 4,
}

unsafe extern "Rust" {
    safe fn dusk_main(handle: u64, user: *mut c_void) -> Result<DuskImplExit, DuskMainFailed>;
}

struct HandleGuard(u64);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unbind_handle(self.0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn dusk_new() -> u64 {
    new_handle()
}

#[unsafe(no_mangle)]
pub extern "C" fn dusk_run(handle: u64, user: *mut c_void) -> i64 {
    let result = match handle::entry(handle)
        .map(|entry| HandleState::from(entry.state.load(Ordering::Acquire)))
    {
        Some(HandleState::Unbound) => {
            let _guard = HandleGuard(handle);
            dusk_main(handle, user)
        }
        Some(HandleState::Binding | HandleState::Bound) => Err(DuskMainFailed::BoundHandle),
        None => Err(DuskMainFailed::UnknownHandle),
    };
    match result {
        Ok(DuskImplExit::Code(exit_code)) => i64::from(exit_code) << 16,
        Ok(DuskImplExit::Panic) => DUSK_RESULT_SOURCE_RUST_PANIC,
        Err(failed) => ((failed as i64) << 16) | DUSK_RESULT_SOURCE_DUSK_MAIN,
    }
}
