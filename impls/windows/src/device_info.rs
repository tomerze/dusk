use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{Kvs, key_id};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

pub(crate) fn set_kvs_device_info(kvs: &Kvs) {
    let values: Vec<(&str, Value)> = [
        ("dusk.device.cores", cores()),
        ("dusk.device.memory_bytes", memory_bytes()),
        ("dusk.device.boot_time_ms", boot_time_ms()),
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

fn memory_bytes() -> Option<Value> {
    let mut status: MEMORYSTATUSEX = unsafe { core::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "GlobalMemoryStatusEx failed");
        return None;
    }
    Some(Value::Uint(status.ullTotalPhys))
}

fn boot_time_ms() -> Option<Value> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;
    let uptime = std::time::Duration::from_millis(unsafe { GetTickCount64() });
    let Some(boot_time) = std::time::SystemTime::now().checked_sub(uptime) else {
        tracing::warn!(uptime = ?uptime, "the uptime is longer than the clock allows");
        return None;
    };
    match boot_time.duration_since(std::time::UNIX_EPOCH) {
        Ok(since_epoch) => Some(Value::Uint(since_epoch.as_millis() as u64)),
        Err(error) => {
            tracing::warn!(error = %error, "the device booted before 1970");
            None
        }
    }
}
