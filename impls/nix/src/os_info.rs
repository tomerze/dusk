use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{FLAG_STICKY, Kvs, key_id};

pub(crate) fn set_kvs_os_info(kvs: &Kvs) {
    set_kvs_process(kvs);
    set_kvs_time_zone(kvs);
    set_kvs_uname(kvs);
    set_kvs_credentials(kvs);
    set_kvs_limits(kvs);
    #[cfg(target_os = "linux")]
    set_kvs_os_release(kvs);
    #[cfg(target_os = "linux")]
    set_kvs_boot_id(kvs);
    #[cfg(target_os = "linux")]
    set_kvs_pid1(kvs);
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    set_kvs_glibc_version(kvs);
    #[cfg(target_os = "android")]
    set_kvs_android_properties(kvs);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    set_kvs_apple_sysctls(kvs);
}

fn set_kvs_values(kvs: &Kvs, source: &str, values: Vec<(String, Value)>) {
    tracing::info!(source, values = ?values, "dusk os");
    for (name, value) in values {
        if let Err(error) = block_on(kvs.set(key_id(&name), value, FLAG_STICKY)) {
            tracing::warn!(name = name.as_str(), error = %format!("{error:#}"), "couldn't record a key in the kvs");
        }
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
    values.push((
        String::from("dusk.os.process.parent_pid"),
        Value::Uint(u64::from(std::os::unix::process::parent_id())),
    ));
    set_kvs_values(kvs, "process", values);
}

fn set_kvs_time_zone(kvs: &Kvs) {
    match iana_time_zone::get_timezone() {
        Ok(time_zone) => set_kvs_values(
            kvs,
            "time zone",
            vec![(String::from("dusk.os.time_zone"), Value::String(time_zone))],
        ),
        Err(error) => tracing::warn!(error = %error, "reading the time zone failed"),
    }
}

fn set_kvs_uname(kvs: &Kvs) {
    let uname = match nix::sys::utsname::uname() {
        Ok(uname) => uname,
        Err(error) => {
            tracing::warn!(error = %error, "uname failed");
            return;
        }
    };
    let fields = [
        ("dusk.os.nix.uname.sysname", uname.sysname()),
        ("dusk.os.nix.uname.nodename", uname.nodename()),
        ("dusk.os.nix.uname.release", uname.release()),
        ("dusk.os.nix.uname.version", uname.version()),
        ("dusk.os.nix.uname.machine", uname.machine()),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        ("dusk.os.nix.uname.domainname", uname.domainname()),
    ];
    let values = fields
        .into_iter()
        .map(|(name, value)| {
            (
                String::from(name),
                Value::String(value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    set_kvs_values(kvs, "uname", values);
}

fn set_kvs_credentials(kvs: &Kvs) {
    let values = vec![
        (
            String::from("dusk.os.nix.uid"),
            Value::Uint(u64::from(unsafe { nix::libc::getuid() })),
        ),
        (
            String::from("dusk.os.nix.euid"),
            Value::Uint(u64::from(unsafe { nix::libc::geteuid() })),
        ),
    ];
    set_kvs_values(kvs, "credentials", values);
}

fn set_kvs_limits(kvs: &Kvs) {
    use nix::sys::resource::{RLIM_INFINITY, Resource, getrlimit};
    let mut values = Vec::new();
    for (name, resource) in [
        ("dusk.os.nix.limits.open_files", Resource::RLIMIT_NOFILE),
        ("dusk.os.nix.limits.core_file_size", Resource::RLIMIT_CORE),
    ] {
        let (soft, hard) = match getrlimit(resource) {
            Ok(limits) => limits,
            Err(error) => {
                tracing::warn!(resource = ?resource, error = %error, "getrlimit failed");
                continue;
            }
        };
        for (bound, limit) in [("soft", soft), ("hard", hard)] {
            let value = if limit == RLIM_INFINITY {
                Value::String(String::from("unlimited"))
            } else {
                #[allow(clippy::unnecessary_cast)]
                let limit = limit as u64;
                Value::Uint(limit)
            };
            values.push((format!("{name}.{bound}"), value));
        }
    }
    set_kvs_values(kvs, "resource limits", values);
}

#[cfg(target_os = "linux")]
fn set_kvs_os_release(kvs: &Kvs) {
    let os_release = match etc_os_release::OsRelease::open() {
        Ok(os_release) => os_release,
        Err(etc_os_release::Error::NoOsRelease) => {
            tracing::info!("no os-release file found");
            return;
        }
        Err(error) => {
            tracing::warn!(error = %error, "reading os-release failed");
            return;
        }
    };
    let values = [
        "NAME",
        "PRETTY_NAME",
        "ID",
        "ID_LIKE",
        "VERSION",
        "VERSION_ID",
        "VERSION_CODENAME",
    ]
    .into_iter()
    .filter_map(|key| {
        os_release.get_value(key).map(|value| {
            (
                format!("dusk.os.linux.os_release.{}", key.to_ascii_lowercase()),
                Value::String(String::from(value)),
            )
        })
    })
    .collect();
    set_kvs_values(kvs, "os-release", values);
}

#[cfg(target_os = "linux")]
fn set_kvs_boot_id(kvs: &Kvs) {
    match std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        Ok(boot_id) => set_kvs_values(
            kvs,
            "boot id",
            vec![(
                String::from("dusk.os.linux.boot_id"),
                Value::String(String::from(boot_id.trim())),
            )],
        ),
        Err(error) => tracing::warn!(error = %error, "reading the boot id failed"),
    }
}

#[cfg(target_os = "linux")]
fn set_kvs_pid1(kvs: &Kvs) {
    match std::fs::read_to_string("/proc/1/comm") {
        Ok(name) => set_kvs_values(
            kvs,
            "pid 1",
            vec![(
                String::from("dusk.os.linux.pid1"),
                Value::String(String::from(name.trim())),
            )],
        ),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            tracing::info!(error = %error, "process 1 is hidden from the node")
        }
        Err(error) => tracing::warn!(error = %error, "reading the name of process 1 failed"),
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn set_kvs_glibc_version(kvs: &Kvs) {
    let version = unsafe { std::ffi::CStr::from_ptr(nix::libc::gnu_get_libc_version()) };
    set_kvs_values(
        kvs,
        "glibc",
        vec![(
            String::from("dusk.os.linux.glibc_version"),
            Value::String(version.to_string_lossy().into_owned()),
        )],
    );
}

#[cfg(target_os = "android")]
fn set_kvs_android_properties(kvs: &Kvs) {
    let properties = android_system_properties::AndroidSystemProperties::new();
    let mut values = Vec::new();
    for (name, property) in [
        ("dusk.os.android.release", "ro.build.version.release"),
        ("dusk.os.android.sdk", "ro.build.version.sdk"),
        (
            "dusk.os.android.security_patch",
            "ro.build.version.security_patch",
        ),
        (
            "dusk.os.android.incremental",
            "ro.build.version.incremental",
        ),
        ("dusk.os.android.model", "ro.product.model"),
        ("dusk.os.android.manufacturer", "ro.product.manufacturer"),
        ("dusk.os.android.fingerprint", "ro.build.fingerprint"),
        ("dusk.os.android.brand", "ro.product.brand"),
        ("dusk.os.android.build_type", "ro.build.type"),
        ("dusk.os.android.abi_list", "ro.product.cpu.abilist"),
    ] {
        let Some(value) = properties.get(property) else {
            tracing::warn!(property, "Android system property not found");
            continue;
        };
        let value = if property == "ro.build.version.sdk" {
            match value.parse::<u64>() {
                Ok(sdk) => Value::Uint(sdk),
                Err(error) => {
                    tracing::warn!(property, value, error = %error, "Android system property is not a number");
                    continue;
                }
            }
        } else {
            Value::String(value)
        };
        values.push((String::from(name), value));
    }
    set_kvs_values(kvs, "Android system properties", values);
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn set_kvs_apple_sysctls(kvs: &Kvs) {
    use sysctl::Sysctl;
    let mut values = Vec::new();
    for (field, name) in [
        ("product_version", "kern.osproductversion"),
        ("build_version", "kern.osversion"),
    ] {
        match sysctl::Ctl::new(name).and_then(|control| control.value_string()) {
            Ok(value) => values.push((
                format!("dusk.os.{}.{field}", std::env::consts::OS),
                Value::String(value),
            )),
            Err(error) => tracing::warn!(name, error = %error, "sysctl failed"),
        }
    }
    #[cfg(target_os = "macos")]
    match sysctl::Ctl::new("sysctl.proc_translated").and_then(|control| control.value_string()) {
        Ok(value) => values.push((
            String::from("dusk.os.macos.translated"),
            Value::Bool(value == "1"),
        )),
        Err(sysctl::SysctlError::NotFound(_)) => {
            values.push((String::from("dusk.os.macos.translated"), Value::Bool(false)))
        }
        Err(error) => {
            tracing::warn!(name = "sysctl.proc_translated", error = %error, "sysctl failed")
        }
    }
    set_kvs_values(kvs, "sysctl", values);
}
