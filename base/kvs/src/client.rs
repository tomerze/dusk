use super::*;
use crate::kvs::key_id;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use linkme::distributed_slice;
use std::rc::Rc;

/// A key name some program registered, paired with the id it hashes to.
#[derive(Copy, Clone)]
pub struct KnownKey {
    pub name: &'static str,
    pub id: u64,
}

/// Every key name registered with [`known_key!`](crate::known_key), collected
/// at link time.
#[distributed_slice]
pub static KNOWN_KEYS: [KnownKey] = [..];

/// The name `id` was hashed from, if a program registered it.
#[must_use]
pub fn known_key_name(id: u64) -> Option<&'static str> {
    KNOWN_KEYS
        .iter()
        .find(|key| key.id == id)
        .map(|key| key.name)
}

/// Register a key name so `kvs get` would know what name to associate with an id.
#[macro_export]
macro_rules! known_key {
    ($binding:ident, $name:literal) => {
        #[$crate::linkme::distributed_slice($crate::client::KNOWN_KEYS)]
        #[linkme(crate = $crate::linkme)]
        static $binding: $crate::client::KnownKey = $crate::client::KnownKey {
            name: $name,
            id: $crate::kvs::key_id($name),
        };
    };
}

#[derive(clap::Parser)]
#[command(name = "kvs", no_binary_name = true)]
struct KvsCli {
    #[command(subcommand)]
    action: KvsAction,
}

#[derive(clap::Subcommand)]
enum KvsAction {
    /// Read the value stored under a key
    Get { key: String },
    /// Store a value under a key
    Set { key: String, value: String },
    /// Remove a key
    Delete { key: String },
    /// Report whether a key is present
    Exists { key: String },
    /// Run no operation and stay alive, serving this process's portal
    Bind,
}

struct KvsProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for KvsProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = KvsCli::try_parse_from(args)?;
        let args = match cli.action {
            KvsAction::Get { key } => Args::get(key_id(&key)),
            KvsAction::Set { key, value } => Args::set(key_id(&key), &Value::String(value))?,
            KvsAction::Delete { key } => Args::delete(key_id(&key)),
            KvsAction::Exists { key } => Args::exists(key_id(&key)),
            KvsAction::Bind => Args::bind(),
        };
        Ok(args.as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(kvs_capnp::PROGRAM_ID),
            name: "kvs",
            short_description: "key-value store",
            long_description: r#"
The `kvs` program reads and writes a key-value store held by the node.
The key-value store is in-memory and shared across all programs on the node.

* `kvs get <key>` prints the value stored under `<key>`, and fails if there is
  none.
* `kvs set <key> <value>` stores `<value>` under `<key>`. Values typed at the
  prompt are stored as strings.
* `kvs delete <key>` removes `<key>` and reports whether it was present.
* `kvs exists <key>` reports whether `<key>` is present.
* `kvs bind` runs no operation and leaves the process running, so a client can
  drive `get`, `set`, `delete` and `exists` over its portal instead. Stop it
  with `kill <pid>`.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(KvsProgramArgsBuilder {}),
    }
}
