#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

use anyhow::{anyhow, Result};
use async_net::TcpListener;
use dusk::dusk_capnp::dusk_capnp::program_args;
use dusk_program::launcher_set;
use dusk_program::namespace::Namespace;
use embassy_executor::Executor;
use embassy_executor::Spawner;
use futures::io::AsyncReadExt;
use log::error;
use static_cell::StaticCell;
use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub use dusk_program::launcher_set::LauncherSet;
pub use dusk_program::launcher_set::LauncherSetBuilder;
pub use dusk_program::launcher_set::StatelessLauncherSetBuilder;

mod driver;

pub fn set_launcher_set_builder(
    launcher_set_builder: impl launcher_set::LauncherSetBuilder + 'static,
) {
    driver::driver().set_launcher_set_builder(launcher_set_builder);
}

thread_local! {
    static INIT_PROGRAM_ARGS: RefCell<Option<program_args::Client>> = const { RefCell::new(None) };
}

pub fn set_init_program_args(program_args: program_args::Client) {
    INIT_PROGRAM_ARGS.with(|args| {
        let mut args_mut = args.borrow_mut();
        if args_mut.is_some() {
            panic!("set_init_program_args called more than once");
        }
        *args_mut = Some(program_args);
    });
}

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

pub fn bootstrap() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("async_io", log::LevelFilter::Info)
        .format_timestamp_nanos()
        .init();
}

pub fn run() {
    let executor = EXECUTOR.init(Executor::new());

    executor.run(|spawner| {
        if let Err(err) = spawner.spawn(init_wrapper()) {
            error!("failed to spawn init task: {err:#?}")
        }
    });
}
