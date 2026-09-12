use crate::interpreter::{Interpreter, Stop};
use crate::sh_capnp;
use alloc::format;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use dusk_program::anyhow;
use dusk_program::embassy_executor;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::process::ProcessContext;

pub(crate) struct State {
    pub(crate) interpreter: Option<Interpreter>,
    pub(crate) active_stops: Vec<Rc<Stop>>,
}

#[embassy_executor::task(pool_size = 16)]
async fn sh_exec_task(
    task_id: Rc<Cell<u32>>,
    pid: u64,
    namespace_id: u64,
    interpreter: Interpreter,
    script_msg: capnp::message::Builder<capnp::message::HeapAllocator>,
    output: dusk_capnp::dusk_capnp::stream::Client,
    stop: Rc<Stop>,
    state: Rc<RefCell<State>>,
    completion: Rc<
        dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, anyhow::Result<()>>,
    >,
    compiler: sh_capnp::compiler::Client,
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
            let reader = script_msg.get_root_as_reader::<sh_capnp::script::Reader>()?;
            interpreter.exec(reader, output, &stop, compiler).await
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
    script: sh_capnp::script::Reader<'_>,
    output: dusk_capnp::dusk_capnp::stream::Client,
    state: Rc<RefCell<State>>,
    stop: Rc<Stop>,
    compiler: sh_capnp::compiler::Client,
) -> capnp::Result<
    Rc<dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, anyhow::Result<()>>>,
> {
    let mut script_msg = capnp::message::Builder::new_default();
    script_msg.set_root::<sh_capnp::script::Owned>(script)?;

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
        script_msg,
        output,
        stop,
        state,
        completion.clone(),
        compiler,
    )
    .map_err(|e| capnp::Error::failed(format!("failed to spawn sh exec task: {e:?}")))?;
    task_id.set(token.id());
    ctx.namespace.spawner.spawn(token);
    Ok(completion)
}
