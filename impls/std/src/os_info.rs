use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{FLAG_STICKY, Kvs, key_id};

pub(crate) fn set_kvs_os_info(kvs: &Kvs) {
    set_kvs_process(kvs);
    #[cfg(not(windows))]
    set_kvs_time_zone(kvs);
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
    #[cfg(unix)]
    values.push((
        String::from("dusk.os.process.parent_pid"),
        Value::Uint(u64::from(std::os::unix::process::parent_id())),
    ));
    set_kvs_values(kvs, "process", values);
}

#[cfg(not(windows))]
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
