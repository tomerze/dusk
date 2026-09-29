use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{Kvs, key_id};
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;

pub(crate) fn set_kvs_os_info(kvs: &Kvs) {
    set_kvs_windows_version(kvs);
}

pub(crate) fn is_not_found(error: &windows_result::Error) -> bool {
    windows_result::WIN32_ERROR::from_error(error)
        == Some(windows_result::WIN32_ERROR(ERROR_FILE_NOT_FOUND))
}

fn set_kvs_values(kvs: &Kvs, source: &str, values: Vec<(String, Value)>) {
    tracing::info!(source, values = ?values, "dusk os");
    for (name, value) in values {
        block_on(kvs.set(key_id(&name), value));
    }
}

fn set_kvs_windows_version(kvs: &Kvs) {
    let version = windows_version::OsVersion::current();
    let mut values = vec![
        (
            String::from("dusk.os.windows.major_version"),
            Value::Uint(u64::from(version.major)),
        ),
        (
            String::from("dusk.os.windows.minor_version"),
            Value::Uint(u64::from(version.minor)),
        ),
        (
            String::from("dusk.os.windows.build_number"),
            Value::Uint(u64::from(version.build)),
        ),
    ];
    match windows_registry::LOCAL_MACHINE
        .options()
        .read()
        .wow64_64()
        .open("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
    {
        Ok(key) => {
            match key.get_u32("UBR") {
                Ok(revision) => values.push((
                    String::from("dusk.os.windows.revision"),
                    Value::Uint(u64::from(revision)),
                )),
                Err(error) if is_not_found(&error) => {
                    tracing::info!("this Windows has no update revision")
                }
                Err(error) => tracing::warn!(error = %error, "reading the Windows revision failed"),
            }
            match key.get_string("EditionID") {
                Ok(edition) => values.push((
                    String::from("dusk.os.windows.edition"),
                    Value::String(edition),
                )),
                Err(error) => tracing::warn!(error = %error, "reading the Windows edition failed"),
            }
            match key.get_string("DisplayVersion") {
                Ok(display_version) => values.push((
                    String::from("dusk.os.windows.display_version"),
                    Value::String(display_version),
                )),
                Err(error) if is_not_found(&error) => {
                    tracing::info!("this Windows has no display version")
                }
                Err(error) => {
                    tracing::warn!(error = %error, "reading the Windows display version failed")
                }
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, "opening the Windows version registry key failed")
        }
    }
    set_kvs_values(kvs, "windows version", values);
}
