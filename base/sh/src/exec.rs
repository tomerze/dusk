use crate::BytecodeMessage;
use crate::interpreter::{Interpreter, Stop};
use alloc::format;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use dusk_capnp::capnp::message::HeapAllocator;
use dusk_program::anyhow;
use dusk_program::embassy_executor;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::process::ProcessContext;

pub(crate) struct State {
    pub(crate) interpreter: Option<Interpreter>,
    pub(crate) active_stops: Vec<Rc<Stop>>,
}

pub(crate) fn log_detached_result(pid: u64, result: &anyhow::Result<()>) {
    match result {
        Ok(()) => tracing::info!(pid, "detached script finished"),
        Err(error) => tracing::error!(
            pid,
            error = %format!("{error:#}"),
            "detached script failed"
        ),
    }
}

#[embassy_executor::task(pool_size = 16)]
async fn sh_exec_task(
    task_id: Rc<Cell<u32>>,
    pid: u64,
    namespace_id: u64,
    interpreter: Interpreter,
    mut script: BytecodeMessage,
    output: dusk_capnp::dusk_capnp::stream::Client,
    stop: Rc<Stop>,
    state: Rc<RefCell<State>>,
    completion: Rc<
        dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, anyhow::Result<()>>,
    >,
) {
    use tracing::Instrument;
    let span = tracing::info_span!(
        "sh_exec",
        __new_task_id__ = task_id.get(),
        pid,
        namespace_id
    );
    async move {
        let result: anyhow::Result<()> = async {
            interpreter
                .exec(
                    script.get_root::<crate::bytecode::Builder>()?.into_reader(),
                    output,
                    &stop,
                )
                .await
        }
        .await;
        state
            .borrow_mut()
            .active_stops
            .retain(|s| !Rc::ptr_eq(s, &stop));
        completion.signal(result);
    }
    .instrument(span)
    .await;
}

pub(crate) fn spawn_sh_exec_task(
    ctx: &ProcessContext,
    interpreter: Interpreter,
    script: crate::bytecode::Reader<'_>,
    output: dusk_capnp::dusk_capnp::stream::Client,
    state: Rc<RefCell<State>>,
    stop: Rc<Stop>,
) -> capnp::Result<
    Rc<dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, anyhow::Result<()>>>,
> {
    let mut owned_script = BytecodeMessage::new(HeapAllocator::new());
    owned_script.set_root::<crate::bytecode::Owned>(script)?;

    state.borrow_mut().active_stops.push(stop.clone());

    let completion: Rc<
        dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, anyhow::Result<()>>,
    > = Rc::new(dusk_program::embassy_sync::signal::Signal::new());

    let task_id = Rc::new(Cell::new(0u32));
    let token = sh_exec_task(
        task_id.clone(),
        ctx.pid,
        ctx.namespace.id,
        interpreter,
        owned_script,
        output,
        stop,
        state,
        completion.clone(),
    )
    .map_err(|e| capnp::Error::failed(format!("failed to spawn sh exec task: {e:?}")))?;
    task_id.set(token.id());
    ctx.namespace.spawner.spawn(token);
    Ok(completion)
}
