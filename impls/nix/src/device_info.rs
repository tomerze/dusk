use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{Kvs, key_id};

pub(crate) fn set_kvs_device_info(kvs: &Kvs) {
    let values: Vec<(&str, Value)> = [
        ("dusk.device.cores", cores()),
        ("dusk.device.memory_bytes", memory_bytes()),
    ]
    .into_iter()
    .filter_map(|(name, value)| Some((name, value?)))
    .collect();
    tracing::info!(values = ?values, "dusk device");
    for (name, value) in values {
        block_on(kvs.set(key_id(name), value));
    }
}

fn cores() -> Option<Value> {
    match std::thread::available_parallelism() {
        Ok(cores) => Some(Value::Uint(cores.get() as u64)),
        Err(error) => {
            tracing::warn!(error = %error, "reading the number of cores failed");
            None
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn sysinfo() -> Option<nix::sys::sysinfo::SysInfo> {
    match nix::sys::sysinfo::sysinfo() {
        Ok(info) => Some(info),
        Err(error) => {
            tracing::warn!(error = %error, "sysinfo failed");
            None
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn memory_bytes() -> Option<Value> {
    Some(Value::Uint(sysinfo()?.ram_total()))
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn memory_bytes() -> Option<Value> {
    use sysctl::Sysctl;
    let value = match sysctl::Ctl::new("hw.memsize").and_then(|control| control.value_string()) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(name = "hw.memsize", error = %error, "sysctl failed");
            return None;
        }
    };
    match value.parse::<u64>() {
        Ok(bytes) => Some(Value::Uint(bytes)),
        Err(error) => {
            tracing::warn!(name = "hw.memsize", value, error = %error, "sysctl is not a number");
            None
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn memory_bytes() -> Option<Value> {
    None
}
