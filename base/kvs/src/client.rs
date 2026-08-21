use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

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
            KvsAction::Get { key } => Args::get(&key),
            KvsAction::Set { key, value } => Args::set(&key, &Value::String(value))?,
            KvsAction::Delete { key } => Args::delete(&key),
            KvsAction::Exists { key } => Args::exists(&key),
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
            short_description: "key-value store (skeleton: operations answer as an empty store)",
            long_description: r#"
The `kvs` command is a key-value store under construction. Its operations are
wired end to end but nothing is stored yet, so every read answers the way an
empty store would: `get` yields null, `exists` and `delete` yield false, and
`set` yields nothing.
* `kvs get <key>` reads the value stored under `<key>`.
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
