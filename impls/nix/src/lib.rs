extern crate alloc;

use alloc::rc::Rc;
use dusk_program::embassy_executor::Executor;
use dusk_program::launcher_set;
use dusk_program::namespace::Namespace;
use dusk_program::program_args::ProgramArgs;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub use dusk_program::launcher_set::BasicLauncherSetBuilder;
pub use dusk_program::launcher_set::LauncherSet;
pub use dusk_program::launcher_set::LauncherSetBuilder;

mod driver;

/// Panic payload `Driver::exit` raises to unwind the executor, carrying the
/// requested exit code so `run` can recover and return it.
pub(crate) struct ExitCode(pub(crate) i32);

pub fn run(
    launcher_set_builder: impl launcher_set::LauncherSetBuilder + 'static,
    init_program_args: Rc<ProgramArgs>,
) -> i32 {
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
                u128::from_le_bytes(seed),
                spawner,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .ok(),
            ));

            driver::driver().set_launcher_set_builder(root.id, launcher_set_builder);
            dusk_core::init::init(root, init_program_args);
        });
    }));

    match outcome {
        Ok(()) => unreachable!("executor.run() should never return"),
        Err(payload) => match payload.downcast::<ExitCode>() {
            Ok(exit_code) => exit_code.0,
            // Not our exit code - a real panic. Return -1 rather than
            // resume_unwind: unwinding across an extern "C" caller is UB.
            Err(_payload) => -1,
        },
    }
}
