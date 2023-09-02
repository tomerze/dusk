#![feature(type_alias_impl_trait)]

extern crate alloc;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use alloc::sync::Arc;
use log::info;

pub mod driver;

pub struct Namespace {
    pub id: u64,
}

impl Namespace {
    pub fn new(id: u64) -> Self {
        let id = id;
        info!("namespace `{}` created", id);
        Namespace { id }
    }
}

#[embassy_executor::task]
pub async fn session(
    namespace: Arc<Mutex<CriticalSectionRawMutex, Namespace>>,
    _spawner: Arc<Mutex<CriticalSectionRawMutex, Spawner>>,
) {
    let mut _namespace_guard = namespace.lock().await;
    info!("session");
}

#[embassy_executor::task]
pub async fn exec(namespace: Arc<Mutex<CriticalSectionRawMutex, Namespace>>) {
    let mut _namespace_guard = namespace.lock().await;
    info!("exec");
}
