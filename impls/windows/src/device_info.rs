use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{FLAG_STICKY, Kvs, key_id};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

pub(crate) fn set_kvs_device_info(kvs: &Kvs) {
    let values: Vec<(&str, Value)> = [
        ("dusk.device.cores", cores()),
        ("dusk.device.memory_bytes", memory_bytes()),
        ("dusk.device.boot_time_ms", boot_time_ms()),
        (
            "dusk.device.vendor",
            registry_string("HARDWARE\\DESCRIPTION\\System\\BIOS", "SystemManufacturer"),
        ),
        (
            "dusk.device.model",
            registry_string("HARDWARE\\DESCRIPTION\\System\\BIOS", "SystemProductName"),
        ),
        (
            "dusk.device.cpu",
            registry_string(
                "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
                "ProcessorNameString",
            ),
        ),
        ("dusk.device.id", device_id()),
    ]
    .into_iter()
    .filter_map(|(name, value)| Some((name, value?)))
    .collect();
    tracing::info!(values = ?values, "dusk device");
    for (name, value) in values {
        block_on(kvs.set(key_id(name), value, FLAG_STICKY));
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

fn registry_string(path: &str, name: &str) -> Option<Value> {
    let key = match windows_registry::LOCAL_MACHINE
        .options()
        .read()
        .wow64_64()
        .open(path)
    {
        Ok(key) => key,
        Err(error) if crate::os_info::is_not_found(&error) => {
            tracing::info!(path, "the device reports no such registry key");
            return None;
        }
        Err(error) => {
            tracing::warn!(path, error = %error, "opening a registry key failed");
            return None;
        }
    };
    match key.get_string(name) {
        Ok(value) => Some(Value::String(String::from(value.trim()))),
        Err(error) if crate::os_info::is_not_found(&error) => {
            tracing::info!(path, name, "the device reports no such registry value");
            None
        }
        Err(error) => {
            tracing::warn!(path, name, error = %error, "reading a registry value failed");
            None
        }
    }
}

fn device_id() -> Option<Value> {
    match machine_uid::get() {
        Ok(id) => {
            let id = id.trim();
            if id.is_empty() {
                tracing::warn!("the device id is empty");
                return None;
            }
            Some(Value::String(String::from(id)))
        }
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            tracing::info!(error = %error, "this system has no device id");
            None
        }
        Err(error) => {
            tracing::warn!(error = %error, "reading the device id failed");
            None
        }
    }
}
