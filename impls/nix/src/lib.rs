#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

use anyhow::Result;
use dusk::dusk_capnp::dusk_capnp::program_args;
use dusk_program::launcher_set;
use dusk_program::namespace::Namespace;
use embassy_executor::Executor;
use log::error;
use static_cell::StaticCell;

pub use dusk_program::launcher_set::LauncherSet;
pub use dusk_program::launcher_set::LauncherSetBuilder;
pub use dusk_program::launcher_set::StatelessLauncherSetBuilder;

mod driver;

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

async fn init(init_program_args: program_args::Client) -> Result<()> {
    let root = alloc::rc::Rc::new(Namespace::new(0));

    let process = dusk::driver::process(root.clone(), init_program_args).await?;

    process.bootstrap().await?;

    Ok(())
}

#[embassy_executor::task]
async fn init_wrapper(init_program_args: program_args::Client) {
    if let Err(err) = init(init_program_args).await {
        error!("init task crashed: {err}");
    }
}

pub fn bootstrap_logging() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("async_io", log::LevelFilter::Info)
        .format_timestamp_nanos()
        .init();
}

pub fn run(
    launcher_set_builder: impl launcher_set::LauncherSetBuilder + 'static,
    init_program_args: program_args::Client,
) {
    driver::driver().set_launcher_set_builder(launcher_set_builder);

    let executor = EXECUTOR.init(Executor::new());

    executor.run(|spawner| {
        if let Err(err) = spawner.spawn(init_wrapper(init_program_args)) {
            error!("failed to spawn init task: {err:#?}")
        }
    });
}
