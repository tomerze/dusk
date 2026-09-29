use dusk_base::dusk_program_init::Args as InitArgs;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

#[cfg(feature = "impl_nix")]
use dusk_nix as dusk_impl;
#[cfg(feature = "impl_std")]
use dusk_std as dusk_impl;
#[cfg(feature = "impl_windows")]
use dusk_windows as dusk_impl;

pub const DUSK_RUN_OK: i32 = 0;
pub const DUSK_RUN_LAUNCHERS_FAILED: i32 = 1;
pub const DUSK_RUN_INIT_ARGS_FAILED: i32 = 2;
pub const DUSK_RUN_UNKNOWN_NAMESPACE: i32 = 3;
pub const DUSK_RUN_ALREADY_RUNNING: i32 = 4;
pub const DUSK_RUN_NOT_SPAWNED: i32 = 5;

pub const DUSK_STATUS_MASK: i32 = 0xff;
pub const DUSK_EXIT_CODE_SHIFT: u32 = 8;

enum State {
    Registered,
    Running,
    Spawned(JoinHandle<i32>),
    Joining,
}

static NAMESPACES: Mutex<BTreeMap<i64, State>> = Mutex::new(BTreeMap::new());

fn namespaces() -> MutexGuard<'static, BTreeMap<i64, State>> {
    NAMESPACES.lock().unwrap_or_else(PoisonError::into_inner)
}

struct User(*mut c_void);

unsafe impl Send for User {}

impl User {
    fn into_inner(self) -> *mut c_void {
        self.0
    }
}

fn exited(exit_code: i32) -> i32 {
    exit_code.wrapping_shl(DUSK_EXIT_CODE_SHIFT) | DUSK_RUN_OK
}

fn draw_id(namespaces: &BTreeMap<i64, State>) -> Option<i64> {
    loop {
        let mut bytes = [0u8; 8];
        if let Err(error) = getrandom::getrandom(&mut bytes) {
            tracing::error!(%error, "couldn't draw a namespace id from the operating system's random source");
            return None;
        }
        let namespace_id = (u64::from_le_bytes(bytes) >> 1) as i64;
        if namespace_id != 0 && !namespaces.contains_key(&namespace_id) {
            return Some(namespace_id);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn dusk_new() -> i64 {
    let mut namespaces = namespaces();
    let Some(namespace_id) = draw_id(&namespaces) else {
        return 0;
    };
    namespaces.insert(namespace_id, State::Registered);
    namespace_id
}

#[unsafe(no_mangle)]
pub extern "C" fn dusk_run(namespace_id: i64, user: *mut c_void) -> i32 {
    if let Err(status) = claim(namespace_id) {
        return status;
    }
    let _claim = Claim(namespace_id);
    run_node(namespace_id, user)
}

#[unsafe(no_mangle)]
pub extern "C" fn dusk_spawn(user: *mut c_void) -> i64 {
    let user = User(user);
    let mut namespaces = namespaces();
    let Some(namespace_id) = draw_id(&namespaces) else {
        return 0;
    };
    let spawned = std::thread::Builder::new()
        .name(format!("dusk-{namespace_id:016x}"))
        .spawn(move || run_node(namespace_id, user.into_inner()));
    match spawned {
        Ok(handle) => {
            namespaces.insert(namespace_id, State::Spawned(handle));
            namespace_id
        }
        Err(error) => {
            tracing::error!(namespace_id, %error, "couldn't start a thread for a node");
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn dusk_join(namespace_id: i64) -> i32 {
    let handle = {
        let mut namespaces = namespaces();
        let Some(state) = namespaces.get_mut(&namespace_id) else {
            return DUSK_RUN_UNKNOWN_NAMESPACE;
        };
        match std::mem::replace(state, State::Joining) {
            State::Spawned(handle) => handle,
            State::Joining => return DUSK_RUN_ALREADY_RUNNING,
            other => {
                *state = other;
                return DUSK_RUN_NOT_SPAWNED;
            }
        }
    };
    let result = handle.join().unwrap_or_else(|_| {
        tracing::error!(namespace_id, "a spawned node's thread panicked");
        exited(-1)
    });
    namespaces().remove(&namespace_id);
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn dusk_free(namespace_id: i64) {
    let mut namespaces = namespaces();
    if matches!(namespaces.get(&namespace_id), Some(State::Registered)) {
        namespaces.remove(&namespace_id);
    }
}

fn claim(namespace_id: i64) -> Result<(), i32> {
    match namespaces().get_mut(&namespace_id) {
        None => Err(DUSK_RUN_UNKNOWN_NAMESPACE),
        Some(state @ State::Registered) => {
            *state = State::Running;
            Ok(())
        }
        Some(_) => Err(DUSK_RUN_ALREADY_RUNNING),
    }
}

struct Claim(i64);

impl Drop for Claim {
    fn drop(&mut self) {
        namespaces().remove(&self.0);
    }
}

fn run_node(namespace_id: i64, _user: *mut c_void) -> i32 {
    let launcher_set = match dusk_base::default_launcher_set() {
        Ok(launcher_set) => launcher_set,
        Err(error) => {
            tracing::error!(namespace_id, %error, "couldn't build the launcher set");
            return DUSK_RUN_LAUNCHERS_FAILED;
        }
    };
    let init_script = dusk_program_sh_compiler_proc::compile_sh!(env!("DUSK_NODE_INIT_SCRIPT"));
    let init_args = match InitArgs::new(&init_script).and_then(|args| args.as_program_args()) {
        Ok(init_args) => init_args,
        Err(error) => {
            tracing::error!(namespace_id, %error, "couldn't build the init program's args");
            return DUSK_RUN_INIT_ARGS_FAILED;
        }
    };
    exited(dusk_impl::run(
        namespace_id as u64,
        move || Ok(launcher_set.clone()),
        init_args,
    ))
}
