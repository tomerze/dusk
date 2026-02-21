#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

use dusk_core::dusk_capnp::dusk_capnp::program_args;
use dusk_program::anyhow::Result;
use dusk_program::launcher_set;
use dusk_program::namespace::Namespace;
use embassy_executor::Executor;
use tracing::error;

pub use dusk_program::launcher_set::LauncherSet;
pub use dusk_program::launcher_set::LauncherSetBuilder;
pub use dusk_program::launcher_set::StatelessLauncherSetBuilder;

mod driver;

async fn init(
    namespace: alloc::rc::Rc<Namespace>,
    init_program_args: program_args::Client,
) -> Result<()> {
    let client = dusk_core::local_client(namespace.clone()).await;
    let mut process_request = client.process_request();
    process_request.get().set_program_args(init_program_args);
    let process = process_request.send().pipeline.get_result();
    process.run_request().send().promise.await?;

    Ok(())
}

#[embassy_executor::task(pool_size = 16)]
async fn init_wrapper(
    namespace: alloc::rc::Rc<Namespace>,
    init_program_args: program_args::Client,
) {
    if let Err(err) = init(namespace, init_program_args).await {
        error!("init task crashed: {err:#?}");
    };
    std::process::exit(0);
}

pub fn bootstrap_logging() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::rfc_3339())
        .init();
}

pub fn run(
    launcher_set_builder: impl launcher_set::LauncherSetBuilder + 'static,
    init_program_args: program_args::Client,
) -> ! {
    // The executor lives for 'static because this function never returns
    // Using Box::leak is explicit about this intent
    let executor = Box::leak(Box::new(Executor::new()));

    executor.run(|spawner| {
        // And so it begins
        let namespace_id = rand::random::<u64>();
        let root = alloc::rc::Rc::new(Namespace::new(namespace_id, spawner));

        driver::driver().set_launcher_set_builder(root.id, launcher_set_builder);
        if let Err(err) = spawner.spawn(init_wrapper(root, init_program_args)) {
            error!("failed to spawn init task: {err:#?}")
        }
    });

    // This function never returns - executor.run() blocks forever
    #[allow(unreachable_code)]
    {
        unreachable!("executor.run() should never return")
    }
}
