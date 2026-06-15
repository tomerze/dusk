use alloc::rc::Rc;
use core::cell::Cell;
use dusk_program::anyhow::Result;
use dusk_program::namespace::Namespace;
use dusk_program::program_args::ProgramArgs;
use tracing::Instrument;
use tracing::info_span;
use tracing::{error, info};

use crate::driver;
use crate::local_client;

async fn run_init_process(
    namespace: Rc<Namespace>,
    init_program_args: Rc<ProgramArgs>,
) -> Result<()> {
    let client = local_client(namespace).await;
    let mut process_request = client.process_request();
    init_program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
    let process_response = process_request.send().promise.await?;
    let process = process_response.get()?;
    let run_request = process.get_result()?.run_request();
    run_request.send().promise.await?;

    Ok(())
}

#[embassy_executor::task(pool_size = 16)]
async fn init_task(
    task_id: Rc<Cell<u32>>,
    namespace: Rc<Namespace>,
    init_program_args: Rc<ProgramArgs>,
) {
    let span = info_span!(
        "init",
        __new_task_id__ = task_id.get(),
        namespace_id = namespace.id
    );
    if let Err(err) = run_init_process(namespace.clone(), init_program_args)
        .instrument(span.clone())
        .await
    {
        span.in_scope(|| error!("init task crashed: {err:#?}"));
    }
    namespace.terminate().await;
    span.in_scope(|| info!("init task exiting"));
    driver::exit(0);
}

pub fn init(namespace: Rc<Namespace>, init_program_args: Rc<ProgramArgs>) {
    let task_id = Rc::new(Cell::new(0));
    match init_task(task_id.clone(), namespace.clone(), init_program_args) {
        Ok(spawn_token) => {
            task_id.set(spawn_token.id());
            namespace.spawner.spawn(spawn_token);
        }
        Err(err) => error!("failed to create init task: {err:#?}"),
    }
}
