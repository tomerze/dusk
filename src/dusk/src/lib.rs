#![feature(type_alias_impl_trait)]

extern crate alloc;

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

    pub async fn session(&mut self) {
        info!("session");
    }

    pub async fn exec(&mut self) {
        info!("exec")
    }
}


#[embassy_executor::task]
pub async fn session(namespace: Arc<Mutex<CriticalSectionRawMutex, Namespace>>) {
    let mut namespace_guard = namespace.lock().await;
    namespace_guard.session().await
}

#[embassy_executor::task]
pub async fn exec(namespace: Arc<Mutex<CriticalSectionRawMutex, Namespace>>) {
    let mut namespace_guard = namespace.lock().await;
    namespace_guard.exec().await
}


// static ROOT_NAMESPACE: StaticCell<Namespace> = StaticCell::new();

// // TODO: Bring rsock and wsock with this function params (probably with Box<dyn ...>)
// #[embassy_executor::task]
// pub async fn root(spawner: Spawner) {
//     info!("Dusk started");

//     ROOT_NAMESPACE.init(Namespace::new(spawner));

//     //  let serv = SSHServer::new(&mut ssh_rxbuf, &mut ssh_txbuf)?;
//     //
//     //     pub async fn run<B: ?Sized, M: RawMutex>(&self,
//     //     rsock: &mut impl asynch::Read,
//     //     wsock: &mut impl asynch::Write,
//     //     b: &Mutex<M, B>) -> Result<()>
// }
