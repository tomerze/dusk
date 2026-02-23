pub const RPC_TRACE_MESSAGE: &str = "rpc call";

#[unsafe(no_mangle)]
fn _embassy_trace_poll_start(executor_id: u32) {
    tracing::trace!(?executor_id, "executor poll");
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_new(executor_id: u32, task_id: u32) {
    tracing::trace!(?executor_id, ?task_id, "task new");
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_end(executor_id: u32, task_id: u32) {
    tracing::trace!(?executor_id, ?task_id, "task end");
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_exec_begin(executor_id: u32, task_id: u32) {
    tracing::trace!(?executor_id, ?task_id, "task exec begin");
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_exec_end(executor_id: u32, task_id: u32) {
    tracing::trace!(?executor_id, ?task_id, "task exec end");
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_ready_begin(executor_id: u32, task_id: u32) {
    tracing::trace!(?executor_id, ?task_id, "task ready");
}

#[unsafe(no_mangle)]
fn _embassy_trace_executor_idle(executor_id: u32) {
    tracing::trace!(?executor_id, "executor idle")
}
