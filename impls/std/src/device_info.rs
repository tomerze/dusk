use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{FLAG_STICKY, Kvs, key_id};

pub(crate) fn set_kvs_device_info(kvs: &Kvs) {
    let values: Vec<(&str, Value)> = [
        ("dusk.device.cores", cores()),
        ("dusk.device.id", device_id()),
    ]
    .into_iter()
    .filter_map(|(name, value)| Some((name, value?)))
    .collect();
    tracing::info!(values = ?values, "dusk device");
    for (name, value) in values {
        if let Err(error) = block_on(kvs.set(key_id(name), value, FLAG_STICKY)) {
            tracing::warn!(name, error = %format!("{error:#}"), "couldn't record a key in the kvs");
        }
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

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    windows,
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
    windows,
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "illumos"
)))]
fn device_id() -> Option<Value> {
    None
}
