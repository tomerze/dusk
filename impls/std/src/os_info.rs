use dusk_program::embassy_futures::block_on;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{FLAG_STICKY, Kvs, key_id};

pub(crate) fn set_kvs_os_info(kvs: &Kvs) {
    set_kvs_process(kvs);
    #[cfg(not(windows))]
    set_kvs_time_zone(kvs);
    set_kvs_locale(kvs);
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

fn set_kvs_locale(kvs: &Kvs) {
    match first_locale(|name| std::env::var(name).ok()) {
        Some(locale) => set_kvs_values(
            kvs,
            "locale",
            vec![(String::from("dusk.os.locale"), Value::String(locale))],
        ),
        None => tracing::info!("the node runs with no locale set"),
    }
}

fn first_locale(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| lookup(name).filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment<'variables>(
        variables: &'variables [(&'variables str, &'variables str)],
    ) -> impl Fn(&str) -> Option<String> + 'variables {
        move |name| {
            variables
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| String::from(*value))
        }
    }

    #[test]
    fn the_locale_is_lc_all_then_lc_messages_then_lang() {
        let every = [
            ("LANG", "en_US.UTF-8"),
            ("LC_MESSAGES", "de_DE.UTF-8"),
            ("LC_ALL", "fr_FR.UTF-8"),
        ];
        assert_eq!(
            first_locale(environment(&every)).as_deref(),
            Some("fr_FR.UTF-8")
        );
        assert_eq!(
            first_locale(environment(&every[..2])).as_deref(),
            Some("de_DE.UTF-8")
        );
        assert_eq!(
            first_locale(environment(&every[..1])).as_deref(),
            Some("en_US.UTF-8")
        );
        assert_eq!(first_locale(environment(&[])), None);
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        assert_eq!(
            first_locale(environment(&[
                ("LC_ALL", ""),
                ("LC_MESSAGES", ""),
                ("LANG", "C.UTF-8")
            ]))
            .as_deref(),
            Some("C.UTF-8")
        );
        assert_eq!(first_locale(environment(&[("LANG", "")])), None);
    }
}
