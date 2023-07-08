extern crate alloc;

use alloc::sync::Arc;

use embassy_executor::Executor;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use static_cell::StaticCell;
use dusk::Namespace;

mod driver;

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("async_io", log::LevelFilter::Info)
        .format_timestamp_nanos()
        .init();

    let executor = EXECUTOR.init(Executor::new());

    executor.run(|spawner| {
        let root =
            Arc::new(Mutex::<CriticalSectionRawMutex, Namespace>::new(Namespace::new(0)));

        spawner.spawn(dusk::session(root.clone())).unwrap();
    });
}
