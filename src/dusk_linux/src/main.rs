extern crate alloc;

use alloc::sync::Arc;

use dusk::Namespace;
use embassy_executor::{Executor, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use static_cell::StaticCell;

mod driver;

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("async_io", log::LevelFilter::Info)
        .format_timestamp_nanos()
        .init();

    let executor = EXECUTOR.init(Executor::new());

    let root = Arc::new(Mutex::<CriticalSectionRawMutex, Namespace>::new(
        Namespace::new(0),
    ));

    executor.run(|spawner| {
        let spawner_mutex = Arc::new(Mutex::<CriticalSectionRawMutex, Spawner>::new(
        spawner));
        let session_task = dusk::session(root.clone(), spawner_mutex.clone());
        let spawner_guard = spawner_mutex.try_lock().unwrap();
        spawner_guard.spawn(session_task).unwrap();
    });
}
