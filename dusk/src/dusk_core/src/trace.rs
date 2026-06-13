pub const RPC_TRACE_MESSAGE: &str = "rpc call";

#[unsafe(no_mangle)]
#[cfg_attr(not(feature = "tracing"), allow(unused_variables))]
fn _embassy_trace_poll_start(executor_id: u32) {
    #[cfg(feature = "tracing")]
    tracing::trace!(executor_id, "executor poll");
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_new(executor_id: u32, task_id: u32) {
    tracing::trace!(executor_id, task_id, "task new");
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_end(executor_id: u32, task_id: u32) {
    tracing::trace!(executor_id, task_id, "task end");
}

#[unsafe(no_mangle)]
#[cfg_attr(not(feature = "tracing"), allow(unused_variables))]
fn _embassy_trace_task_exec_begin(executor_id: u32, task_id: u32) {
    #[cfg(feature = "tracing")]
    tracing::trace!(executor_id, task_id, "task exec begin");
}

#[unsafe(no_mangle)]
#[cfg_attr(not(feature = "tracing"), allow(unused_variables))]
fn _embassy_trace_task_exec_end(executor_id: u32, task_id: u32) {
    #[cfg(feature = "tracing")]
    tracing::trace!(executor_id, task_id, "task exec end");
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_ready_begin(_executor_id: u32, _task_id: u32) {
    // Deliberately silent: this hook runs inside `wake()`, and wakes happen
    // in arbitrary foreign contexts — the embassy-time std driver wakes tasks
    // from its alarm thread while holding its internal mutex, and a tracing
    // event from here reaches the log buffer's writer, whose timestamping
    // calls `Instant::now()` and takes that same mutex: the node deadlocks.
    // Wakers must stay cheap and lock-free; never trace from wake context.
}

#[unsafe(no_mangle)]
#[cfg_attr(not(feature = "tracing"), allow(unused_variables))]
fn _embassy_trace_executor_idle(executor_id: u32) {
    #[cfg(feature = "tracing")]
    tracing::trace!(executor_id, "executor idle")
}
