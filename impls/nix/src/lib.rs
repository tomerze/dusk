#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

use anyhow::{anyhow, Result};
use dusk::dusk_capnp::dusk_capnp::program_args;
use dusk_program::launcher_set;
use dusk_program::namespace::Namespace;
use embassy_executor::Executor;
use log::error;
use static_cell::StaticCell;
use std::cell::RefCell;

pub use dusk_program::launcher_set::LauncherSet;
pub use dusk_program::launcher_set::LauncherSetBuilder;
pub use dusk_program::launcher_set::StatelessLauncherSetBuilder;

mod driver;

thread_local! {
    static INIT_PROGRAM_ARGS: RefCell<Option<program_args::Client>> = const { RefCell::new(None) };
}

pub fn configure(
    launcher_set_builder: impl launcher_set::LauncherSetBuilder + 'static,
    init_program_args: impl program_args::Server + 'static,
) {
    driver::driver().set_launcher_set_builder(launcher_set_builder);

    INIT_PROGRAM_ARGS.with(|args| {
        let mut args_mut = args.borrow_mut();
        if args_mut.is_some() {
            panic!("configure called more than once");
        }
        *args_mut = Some(capnp_rpc::new_client(init_program_args));
    });
}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

async fn init() -> Result<()> {
    let root = alloc::rc::Rc::new(Namespace::new(0));
    let init_program_args = INIT_PROGRAM_ARGS.with(|args| {
        args.borrow().as_ref().cloned().ok_or_else(|| {
            anyhow!("init program args not configured - call `configure` before `run`")
        })
    })?;
    let process = dusk::driver::process(root.clone(), init_program_args).await?;

    process.bootstrap().await?;

    Ok(())
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
