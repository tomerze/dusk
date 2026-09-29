use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{Kvs, key_id};

pub(crate) fn set_kvs_os_info(kvs: &Kvs) {
    set_kvs_uname(kvs);
    #[cfg(target_os = "linux")]
    set_kvs_os_release(kvs);
    #[cfg(target_os = "android")]
    set_kvs_android_properties(kvs);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    set_kvs_apple_sysctls(kvs);
}

fn set_kvs_values(kvs: &Kvs, source: &str, values: Vec<(String, Value)>) {
    tracing::info!(source, values = ?values, "dusk os");
    for (name, value) in values {
        block_on(kvs.set(key_id(&name), value));
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
