use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{Kvs, key_id};
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;

pub(crate) fn set_kvs_os_info(kvs: &Kvs) {
    set_kvs_process(kvs);
    set_kvs_time_zone(kvs);
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

fn set_kvs_process(kvs: &Kvs) {
    let mut values = vec![(
        String::from("dusk.os.process.pid"),
        Value::Uint(u64::from(std::process::id())),
    )];
    match std::env::current_exe() {
        Ok(executable) => values.push((
            String::from("dusk.os.process.executable"),
            Value::String(executable.to_string_lossy().into_owned()),
        )),
        Err(error) => tracing::warn!(error = %error, "reading the executable path failed"),
    }
    match std::env::current_dir() {
        Ok(directory) => values.push((
            String::from("dusk.os.process.working_directory"),
            Value::String(directory.to_string_lossy().into_owned()),
        )),
        Err(error) => tracing::warn!(error = %error, "reading the working directory failed"),
    }
    if let Some(parent_pid) = parent_pid() {
        values.push((
            String::from("dusk.os.process.parent_pid"),
            Value::Uint(u64::from(parent_pid)),
        ));
    }
    set_kvs_values(kvs, "process", values);
}

fn parent_pid() -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        tracing::warn!(error = %std::io::Error::last_os_error(), "CreateToolhelp32Snapshot failed");
        return None;
    }
    let pid = std::process::id();
    let mut entry: PROCESSENTRY32W = unsafe { core::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut parent_pid = None;
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32ProcessID == pid {
            parent_pid = Some(entry.th32ParentProcessID);
            break;
        }
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    if parent_pid.is_none() {
        tracing::warn!(
            pid,
            "the node's own process is missing from the process list"
        );
    }
    parent_pid
}

fn set_kvs_time_zone(kvs: &Kvs) {
    use windows_sys::Win32::System::Time::{
        DYNAMIC_TIME_ZONE_INFORMATION, GetDynamicTimeZoneInformation, TIME_ZONE_ID_INVALID,
    };
    let mut information: DYNAMIC_TIME_ZONE_INFORMATION = unsafe { core::mem::zeroed() };
    if unsafe { GetDynamicTimeZoneInformation(&mut information) } == TIME_ZONE_ID_INVALID {
        tracing::warn!(error = %std::io::Error::last_os_error(), "GetDynamicTimeZoneInformation failed");
        return;
    }
    let name = &information.TimeZoneKeyName;
    let length = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    set_kvs_values(
        kvs,
        "time zone",
        vec![(
            String::from("dusk.os.time_zone"),
            Value::String(String::from_utf16_lossy(&name[..length])),
        )],
    );
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
