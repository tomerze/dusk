use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{Kvs, key_id};

pub(crate) fn set_kvs_os_info(kvs: &Kvs) {
    set_kvs_uname(kvs);
    #[cfg(target_os = "linux")]
    set_kvs_os_release(kvs);
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
