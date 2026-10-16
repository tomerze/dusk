use alloc::vec::Vec;
use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use dusk_program::embassy_executor::Spawner;
use dusk_program::embassy_executor::raw::{AvailableTask, TaskStorage};
use dusk_program::embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::lazy_lock::LazyLock;
use dusk_program::hashbrown::HashMap;
use nohash_hasher::BuildNoHashHasher;

type Storage = TaskStorage<Pin<Box<dyn Future<Output = ()>>>>;

type Storages = BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<HashMap<usize, Vec<&'static Storage>, BuildNoHashHasher<usize>>>,
>;

static STORAGES: LazyLock<Storages> =
    LazyLock::new(|| BlockingMutex::new(RefCell::new(HashMap::default())));

pub fn spawn(spawner: Spawner, future: impl Future<Output = ()> + 'static) -> u32 {
    let future: Pin<Box<dyn Future<Output = ()>>> = Box::pin(future);
    let (task, storage_count) = STORAGES.get().lock(|executors| {
        let mut executors = executors.borrow_mut();
        let storages = executors.entry(spawner.executor_id()).or_default();
        let mut allocated = false;
        loop {
            if let Some(task) = storages.iter().copied().find_map(AvailableTask::claim) {
                break (task, allocated.then_some(storages.len()));
            }
            storages.push(Box::leak(Box::new(TaskStorage::new())));
            allocated = true;
        }
    });
    if let Some(storage_count) = storage_count {
        tracing::debug!(storage_count, "allocated a task storage");
    }
    let spawn_token = task.initialize(move || future);
    let task_id = spawn_token.id();
    spawner.spawn(spawn_token);
    task_id
}
