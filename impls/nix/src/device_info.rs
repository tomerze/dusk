use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{Kvs, key_id};

pub(crate) fn set_kvs_device_info(kvs: &Kvs) {
    let values: Vec<(&str, Value)> = [
        ("dusk.device.cores", cores()),
        ("dusk.device.memory_bytes", memory_bytes()),
        ("dusk.device.swap_bytes", swap_bytes()),
        ("dusk.device.boot_time_ms", boot_time_ms()),
        ("dusk.device.vendor", vendor()),
        ("dusk.device.model", model()),
        ("dusk.device.cpu", cpu()),
        ("dusk.device.id", device_id()),
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

#[cfg(any(target_os = "linux", target_os = "android"))]
fn swap_bytes() -> Option<Value> {
    Some(Value::Uint(sysinfo()?.swap_total()))
}

#[cfg(target_os = "macos")]
fn swap_bytes() -> Option<Value> {
    let usage: nix::libc::xsw_usage = sysctl_struct("vm.swapusage")?;
    Some(Value::Uint(usage.xsu_total))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn swap_bytes() -> Option<Value> {
    None
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn boot_time_ms() -> Option<Value> {
    let uptime = sysinfo()?.uptime();
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

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn boot_time_ms() -> Option<Value> {
    let boot_time: nix::libc::timeval = sysctl_struct("kern.boottime")?;
    let (Ok(seconds), Ok(microseconds)) = (
        u64::try_from(boot_time.tv_sec),
        u64::try_from(boot_time.tv_usec),
    ) else {
        tracing::warn!(name = "kern.boottime", "sysctl holds a negative time");
        return None;
    };
    Some(Value::Uint(seconds * 1000 + microseconds / 1000))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn boot_time_ms() -> Option<Value> {
    None
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn sysctl_struct<T: Copy>(name: &str) -> Option<T> {
    use sysctl::{CtlValue, Sysctl};
    let bytes = match sysctl::Ctl::new(name).and_then(|control| control.value()) {
        Ok(CtlValue::Struct(bytes) | CtlValue::Node(bytes)) => bytes,
        Ok(_) => {
            tracing::warn!(name, "sysctl is not a struct");
            return None;
        }
        Err(error) => {
            tracing::warn!(name, error = %error, "sysctl failed");
            return None;
        }
    };
    if bytes.len() != size_of::<T>() {
        tracing::warn!(
            name,
            length = bytes.len(),
            expected = size_of::<T>(),
            "sysctl has an unexpected size"
        );
        return None;
    }
    Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) })
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn sysctl_string(name: &str) -> Option<Value> {
    use sysctl::Sysctl;
    match sysctl::Ctl::new(name).and_then(|control| control.value_string()) {
        Ok(value) => Some(Value::String(value)),
        Err(error) => {
            tracing::warn!(name, error = %error, "sysctl failed");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn read_device_string(path: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let contents = contents
                .trim_matches(|character: char| character.is_whitespace() || character == '\0');
            (!contents.is_empty()).then(|| String::from(contents))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(path, error = %error, "reading a device file failed");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn vendor() -> Option<Value> {
    let vendor = read_device_string("/sys/class/dmi/id/sys_vendor");
    if vendor.is_none() {
        tracing::info!("the device reports no vendor");
    }
    vendor.map(Value::String)
}

#[cfg(target_os = "linux")]
fn model() -> Option<Value> {
    let model = read_device_string("/sys/class/dmi/id/product_name")
        .or_else(|| read_device_string("/sys/firmware/devicetree/base/model"));
    if model.is_none() {
        tracing::info!("the device reports no model");
    }
    model.map(Value::String)
}

#[cfg(target_os = "linux")]
fn cpu() -> Option<Value> {
    let cpuinfo = match std::fs::read_to_string("/proc/cpuinfo") {
        Ok(cpuinfo) => cpuinfo,
        Err(error) => {
            tracing::warn!(error = %error, "reading /proc/cpuinfo failed");
            return None;
        }
    };
    let model = cpuinfo.lines().find_map(|line| {
        let (field, value) = line.split_once(':')?;
        (field.trim() == "model name").then(|| String::from(value.trim()))
    });
    if model.is_none() {
        tracing::info!("/proc/cpuinfo names no CPU model");
    }
    model.map(Value::String)
}

#[cfg(target_os = "android")]
fn android_property(property: &str) -> Option<Value> {
    let value = android_system_properties::AndroidSystemProperties::new().get(property);
    if value.is_none() {
        tracing::info!(property, "Android system property not found");
    }
    value.map(Value::String)
}

#[cfg(target_os = "android")]
fn vendor() -> Option<Value> {
    android_property("ro.product.manufacturer")
}

#[cfg(target_os = "android")]
fn model() -> Option<Value> {
    android_property("ro.product.model")
}

#[cfg(target_os = "android")]
fn cpu() -> Option<Value> {
    android_property("ro.soc.model")
}

#[cfg(target_os = "macos")]
fn model() -> Option<Value> {
    sysctl_string("hw.model")
}

#[cfg(target_os = "ios")]
fn model() -> Option<Value> {
    sysctl_string("hw.machine")
}

#[cfg(target_os = "macos")]
fn cpu() -> Option<Value> {
    sysctl_string("machdep.cpu.brand_string")
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn vendor() -> Option<Value> {
    None
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn model() -> Option<Value> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn cpu() -> Option<Value> {
    None
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "illumos"
))]
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

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "illumos"
)))]
fn device_id() -> Option<Value> {
    None
}
