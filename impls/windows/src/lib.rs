extern crate alloc;

use alloc::rc::Rc;
use dusk_program::embassy_executor::Executor;
use dusk_program::namespace::Namespace;
use dusk_program::program_args::ProgramArgs;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub use dusk_program::launcher_set::LauncherSet;

mod driver;

thread_local! {
    static EXIT_CODE: core::cell::Cell<Option<i32>> = const { core::cell::Cell::new(None) };
}

pub(crate) fn exit(exit_code: i32) {
    EXIT_CODE.with(|exit| exit.set(Some(exit_code)));
}

pub fn run(
    namespace_id: u64,
    launcher_set: impl Fn() -> dusk_program::anyhow::Result<LauncherSet> + Send + Sync + 'static,
    init_program_args: Rc<ProgramArgs>,
) -> i32 {
    EXIT_CODE.with(|exit| exit.set(None));

    let executor = Box::leak(Box::new(Executor::new()));

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        executor.run_until(
            |spawner| {
                let mut seed = [0u8; 16];
                getrandom::getrandom(&mut seed)
                    .expect("the operating system's random source failed");

                let random_seed = u128::from_le_bytes(seed);
                let unix_time_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .ok();
                let root = Rc::new(Namespace::new(
                    namespace_id,
                    random_seed,
                    spawner,
                    unix_time_ms,
                ));

                dusk_core::init::init(root, launcher_set, init_program_args);
            },
            || EXIT_CODE.with(|exit| exit.get().is_some()),
        );
    }));

    dusk_core::launchers::remove_launcher_set(namespace_id);

    match outcome {
        Ok(()) => EXIT_CODE.with(|exit| exit.take()).unwrap_or(-1),
        Err(_payload) => -1,
    }
}
