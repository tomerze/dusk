extern crate alloc;

use alloc::rc::Rc;
use dusk_core::driver::DuskImplExit;
use dusk_program::embassy_executor::Executor;
use dusk_program::namespace::Namespace;
use dusk_program::program_args::ProgramArgs;
use dusk_program::value::Value;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub use dusk_program::launcher_set::LauncherSet;

mod device_info;
mod driver;
mod os_info;

/// Panic payload `Driver::exit` raises to unwind the executor, carrying the
/// requested exit code so `run` can recover and return it.
pub(crate) struct ExitCode(pub(crate) i32);

pub fn run(
    handle: u64,
    launcher_set: impl Fn() -> dusk_program::anyhow::Result<LauncherSet> + Send + Sync + 'static,
    init_program_args: Rc<ProgramArgs>,
) -> DuskImplExit {
    // Keep the exit-code panic out of the default panic output so a clean
    // exit() doesn't look like a crash. Real panics still print normally.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        if panic_info.payload().is::<ExitCode>() {
            return;
        }
        default_hook(panic_info);
    }));

    // Box::leak gives the executor a 'static borrow, as Executor::run requires.
    let executor = Box::leak(Box::new(Executor::new()));

    // Normally executor.run() blocks forever, so this returns only when a
    // process unwinds it via panic (e.g. Driver::exit).
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        executor.run(|spawner| {
            // And so it begins
            let mut seed = [0u8; 16];
            getrandom::getrandom(&mut seed).expect("the operating system's random source failed");

            let root = Rc::new(Namespace::new(
                handle,
                u128::from_le_bytes(seed),
                spawner,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .ok(),
            ));

            let kvs = dusk_program_kvs_internal::get_kvs(root.id);
            dusk_program::embassy_futures::block_on(kvs.set(
                dusk_program_kvs_internal::key_id("dusk.impl"),
                Value::String(String::from("nix")),
                dusk_program_kvs_internal::FLAG_STICKY,
            ));
            os_info::set_kvs_os_info(&kvs);
            device_info::set_kvs_device_info(&kvs);

            dusk_core::init::init(
                root,
                move || {
                    let _store = &kvs;
                    launcher_set()
                },
                init_program_args,
            );
        });
    }));

    match outcome {
        Ok(()) => unreachable!("executor.run() should never return"),
        Err(payload) => match payload.downcast::<ExitCode>() {
            Ok(exit_code) => DuskImplExit::Code(exit_code.0),
            // Not our exit code - a real panic. Return Panic rather than
            // resume_unwind: unwinding across an extern "C" caller is UB.
            Err(_payload) => DuskImplExit::Panic,
        },
    }
}
