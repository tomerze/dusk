#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

use anyhow::{anyhow, Result};
use async_net::TcpListener;
use dusk_program::namespace::Namespace;
use embassy_executor::Executor;
use embassy_executor::Spawner;
use futures::io::AsyncReadExt;
use log::error;
use static_cell::StaticCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

mod driver;

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

async fn init() -> Result<()> {
    let root = alloc::rc::Rc::new(Namespace::new(0));
    loop {
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 9090)).await?;
        let (stream, _) = listener.accept().await?;
        stream.set_nodelay(true)?;
        let (reader, writer) = stream.split();
        let session_task = dusk::session(root.clone(), Box::pin(reader), Box::pin(writer));
        let spawner = unsafe { Spawner::for_current_executor().await };
        spawner
            .spawn(session_task)
            .map_err(|err| anyhow!("failed to spawn session task {err:#?}"))?;
    }
}

#[embassy_executor::task]
async fn init_wrapper() {
    if let Err(err) = init().await {
        error!("init task crashed: {err}");
    }
}

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("async_io", log::LevelFilter::Info)
        .format_timestamp_nanos()
        .init();

    let executor = EXECUTOR.init(Executor::new());

    executor.run(|spawner| {
        if let Err(err) = spawner.spawn(init_wrapper()) {
            error!("failed to spawn init task: {err:#?}")
        }
    });
}
