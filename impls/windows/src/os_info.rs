use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{FLAG_STICKY, Kvs, key_id};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::SystemInformation::{
    GetNativeSystemInfo, IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64,
    IMAGE_FILE_MACHINE_ARMNT, IMAGE_FILE_MACHINE_I386, PROCESSOR_ARCHITECTURE_AMD64,
    PROCESSOR_ARCHITECTURE_ARM, PROCESSOR_ARCHITECTURE_ARM64, PROCESSOR_ARCHITECTURE_INTEL,
    SYSTEM_INFO,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::core::BOOL;

pub(crate) fn set_kvs_os_info(kvs: &Kvs) {
    set_kvs_process(kvs);
    set_kvs_time_zone(kvs);
    set_kvs_windows_version(kvs);
    set_kvs_windows_emulation(kvs);
    set_kvs_windows_computer_name(kvs);
    set_kvs_windows_session(kvs);
}

pub(crate) fn is_not_found(error: &windows_result::Error) -> bool {
    windows_result::WIN32_ERROR::from_error(error)
        == Some(windows_result::WIN32_ERROR(ERROR_FILE_NOT_FOUND))
}

fn set_kvs_values(kvs: &Kvs, source: &str, values: Vec<(String, Value)>) {
    tracing::info!(source, values = ?values, "dusk os");
    for (name, value) in values {
        block_on(kvs.set(key_id(&name), value, FLAG_STICKY));
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

fn set_kvs_windows_emulation(kvs: &Kvs) {
    let native_arch = match native_machine() {
        Some(Ok(native_machine)) => match native_machine {
            IMAGE_FILE_MACHINE_I386 => Some("x86"),
            IMAGE_FILE_MACHINE_AMD64 => Some("x86_64"),
            IMAGE_FILE_MACHINE_ARMNT => Some("arm"),
            IMAGE_FILE_MACHINE_ARM64 => Some("aarch64"),
            unknown => {
                tracing::warn!(
                    native_machine = unknown,
                    "IsWow64Process2 reported a native machine Dusk does not know"
                );
                None
            }
        },
        Some(Err(error)) => {
            tracing::warn!(error = %error, "IsWow64Process2 failed");
            None
        }
        None => {
            let mut system_info: SYSTEM_INFO = unsafe { core::mem::zeroed() };
            unsafe { GetNativeSystemInfo(&mut system_info) };
            match unsafe { system_info.Anonymous.Anonymous.wProcessorArchitecture } {
                PROCESSOR_ARCHITECTURE_INTEL => Some("x86"),
                PROCESSOR_ARCHITECTURE_AMD64 => Some("x86_64"),
                PROCESSOR_ARCHITECTURE_ARM => Some("arm"),
                PROCESSOR_ARCHITECTURE_ARM64 => Some("aarch64"),
                unknown => {
                    tracing::warn!(
                        processor_architecture = unknown,
                        "GetNativeSystemInfo reported a processor architecture Dusk does not know"
                    );
                    None
                }
            }
        }
    };
    let Some(native_arch) = native_arch else {
        return;
    };
    set_kvs_values(
        kvs,
        "windows emulation",
        vec![
            (
                String::from("dusk.os.windows.native_arch"),
                Value::String(String::from(native_arch)),
            ),
            (
                String::from("dusk.os.windows.emulated"),
                Value::Bool(native_arch != std::env::consts::ARCH),
            ),
        ],
    );
}

fn native_machine() -> Option<std::io::Result<IMAGE_FILE_MACHINE>> {
    type IsWow64Process2 =
        unsafe extern "system" fn(HANDLE, *mut IMAGE_FILE_MACHINE, *mut IMAGE_FILE_MACHINE) -> BOOL;
    let kernel32: Vec<u16> = "kernel32.dll".encode_utf16().chain([0]).collect();
    let module = unsafe { GetModuleHandleW(kernel32.as_ptr()) };
    if module.is_null() {
        return None;
    }
    let function = unsafe { GetProcAddress(module, c"IsWow64Process2".as_ptr().cast()) }?;
    let is_wow64_process2 = unsafe {
        core::mem::transmute::<unsafe extern "system" fn() -> isize, IsWow64Process2>(function)
    };
    let mut process_machine: IMAGE_FILE_MACHINE = 0;
    let mut native_machine: IMAGE_FILE_MACHINE = 0;
    if unsafe {
        is_wow64_process2(
            GetCurrentProcess(),
            &mut process_machine,
            &mut native_machine,
        )
    } == 0
    {
        return Some(Err(std::io::Error::last_os_error()));
    }
    Some(Ok(native_machine))
}

fn set_kvs_windows_computer_name(kvs: &Kvs) {
    use windows_sys::Win32::System::WindowsProgramming::{
        GetComputerNameW, MAX_COMPUTERNAME_LENGTH,
    };
    let mut name = [0u16; MAX_COMPUTERNAME_LENGTH as usize + 1];
    let mut length = name.len() as u32;
    if unsafe { GetComputerNameW(name.as_mut_ptr(), &mut length) } == 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "GetComputerNameW failed");
        return;
    }
    set_kvs_values(
        kvs,
        "computer name",
        vec![(
            String::from("dusk.os.windows.computer_name"),
            Value::String(String::from_utf16_lossy(&name[..length as usize])),
        )],
    );
}

fn set_kvs_windows_session(kvs: &Kvs) {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    let mut values = Vec::new();
    let mut session_id = 0u32;
    if unsafe { ProcessIdToSessionId(std::process::id(), &mut session_id) } == 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "ProcessIdToSessionId failed");
    } else {
        values.push((
            String::from("dusk.os.windows.session_id"),
            Value::Uint(u64::from(session_id)),
        ));
    }
    if let Some(elevated) = elevated() {
        values.push((
            String::from("dusk.os.windows.elevated"),
            Value::Bool(elevated),
        ));
    }
    set_kvs_values(kvs, "windows session", values);
}

fn elevated() -> Option<bool> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::OpenProcessToken;
    let mut token: HANDLE = core::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "OpenProcessToken failed");
        return None;
    }
    let mut elevation: TOKEN_ELEVATION = unsafe { core::mem::zeroed() };
    let mut length = 0u32;
    let succeeded = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    } != 0;
    let error = (!succeeded).then(std::io::Error::last_os_error);
    unsafe { CloseHandle(token) };
    if let Some(error) = error {
        tracing::warn!(error = %error, "GetTokenInformation failed");
        return None;
    }
    Some(elevation.TokenIsElevated != 0)
}
