#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

use alloc::rc::Rc;
use core::cell::Cell;
use dusk_program::anyhow::Result;
use dusk_program::embassy_executor::Executor;
use dusk_program::launcher_set;
use dusk_program::namespace::Namespace;
use dusk_program::program_args::ProgramArgs;
use tracing::Instrument;
use tracing::error;

pub use dusk_program::launcher_set::BasicLauncherSetBuilder;
pub use dusk_program::launcher_set::LauncherSet;
pub use dusk_program::launcher_set::LauncherSetBuilder;
use tracing::info_span;

mod driver;

async fn init(namespace: Rc<Namespace>, init_program_args: Rc<ProgramArgs>) -> Result<()> {
    let client = dusk_core::local_client(namespace.clone()).await;
    let mut process_request = client.process_request();
    init_program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
    let process_response = process_request.send().promise.await?;
    let process = process_response.get()?;
    let mut run_request = client.run_request();
    run_request.get().set_process(process.get_result()?);
    run_request.send().promise.await?;

    Ok(())
}

#[embassy_executor::task(pool_size = 16)]
async fn init_wrapper(
    task_id: Rc<Cell<u32>>,
    namespace: Rc<Namespace>,
    init_program_args: Rc<ProgramArgs>,
) {
    let span = info_span!("init", task_id = task_id.get(), namespace_id = namespace.id);
    if let Err(err) = init(namespace, init_program_args)
        .instrument(span.clone())
        .await
    {
        span.in_scope(|| error!("init task crashed: {err:#?}"));
    };
}

pub fn bootstrap_logging() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::rfc_3339())
        .init();
}

pub fn run(
    launcher_set_builder: impl launcher_set::LauncherSetBuilder + 'static,
    init_program_args: Rc<ProgramArgs>,
) -> ! {
    // The executor lives for 'static because this function never returns
    // Using Box::leak is explicit about this intent
    let executor = Box::leak(Box::new(Executor::new()));

    executor.run(|spawner| {
        // And so it begins
        let namespace_id = rand::random::<u64>();
        let root = Rc::new(Namespace::new(namespace_id, spawner));

        driver::driver().set_launcher_set_builder(root.id, launcher_set_builder);
        let task_id = Rc::new(Cell::new(0));
        match init_wrapper(task_id.clone(), root, init_program_args) {
            Ok(spawn_token) => {
                task_id.set(spawn_token.id());
                spawner.spawn(spawn_token);
            }
            Err(err) => error!("failed to create init task: {err:#?}"),
        }
    });

    // This function never returns - executor.run() blocks forever
    #[allow(unreachable_code)]
    {
        unreachable!("executor.run() should never return")
    }
}
