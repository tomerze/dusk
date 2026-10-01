use portable_atomic::{AtomicU8, AtomicU64, Ordering};
use tracing::warn;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleState {
    Unbound = 0,
    Binding = 1,
    Bound = 2,
}

impl From<u8> for HandleState {
    fn from(value: u8) -> Self {
        match value {
            0 => HandleState::Unbound,
            1 => HandleState::Binding,
            _ => HandleState::Bound,
        }
    }
}

pub struct HandleEntry {
    pub state: AtomicU8,
    pub namespace_id: AtomicU64,
}

static HANDLES: boxcar::Vec<HandleEntry> = boxcar::Vec::new();

pub fn entry(handle: u64) -> Option<&'static HandleEntry> {
    HANDLES.get(usize::try_from(handle.checked_sub(1)?).ok()?)
}

pub fn new_handle() -> u64 {
    HANDLES.push(HandleEntry {
        state: AtomicU8::new(HandleState::Unbound as u8),
        namespace_id: AtomicU64::new(0),
    }) as u64
        + 1
}

pub fn bind_handle(handle: u64, namespace_id: u64) {
    if let Some(entry) = entry(handle) {
        match entry.state.compare_exchange(
            HandleState::Unbound as u8,
            HandleState::Binding as u8,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                entry.namespace_id.store(namespace_id, Ordering::Relaxed);
                entry
                    .state
                    .store(HandleState::Bound as u8, Ordering::Release);
            }
            Err(state) => warn!(
                handle,
                namespace_id,
                state = ?HandleState::from(state),
                "namespace created under a handle that is not unbound"
            ),
        }
    }
}

pub fn unbind_handle(handle: u64) {
    if let Some(entry) = entry(handle) {
        entry
            .state
            .store(HandleState::Unbound as u8, Ordering::Release);
    }
}

pub fn namespace_id(handle: u64) -> Option<u64> {
    let entry = entry(handle)?;
    match HandleState::from(entry.state.load(Ordering::Acquire)) {
        HandleState::Bound => Some(entry.namespace_id.load(Ordering::Relaxed)),
        _ => None,
    }
}
