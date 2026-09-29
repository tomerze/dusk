use super::*;
use crate::kvs::key_id;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program::stream::{Stream, StreamMixin};
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::borrow::ToOwned;
use std::cell::RefCell;
use std::rc::Rc;

/// How a key id is shown to a user: the name the program that writes it
/// registered, else `0x…` hex.
#[must_use]
pub fn key_display(id: u64) -> String {
    crate::kvs::known_key_name(id).map_or_else(|| format!("{id:#018x}"), str::to_owned)
}

/// The id a user-typed key refers to: a `0x…` hex id as [`key_display`] shows
/// one, else the hash of the name.
#[must_use]
pub fn key_parse(key: &str) -> u64 {
    key.strip_prefix("0x")
        .or_else(|| key.strip_prefix("0X"))
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .unwrap_or_else(|| key_id(key))
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
    Get {
        key: String,
    },
    /// Store a value under a key
    Set {
        key: String,
        value: String,
    },
    /// Remove a key
    Delete {
        key: String,
    },
    /// Report whether a key is present
    Exists {
        key: String,
    },
    // Bind kvs on the client as a redis-compatible server
    Bind,
    /// List every key, by name where a program registered one
    Scan,
}

struct ScannedKeys {
    keys: Rc<RefCell<Vec<u64>>>,
}

impl StreamMixin for ScannedKeys {
    fn send(&mut self, value: Value) -> Promise<(), capnp::Error> {
        let Value::List(page) = value else {
            return Promise::err(capnp::Error::failed(format!(
                "kvs scan sent {value:?} where a page of key ids belongs"
            )));
        };
        for key in page {
            let Value::Uint(key) = key else {
                return Promise::err(capnp::Error::failed(format!(
                    "kvs scan sent {key:?} where a key id belongs"
                )));
            };
            self.keys.borrow_mut().push(key);
        }
        Promise::ok(())
    }

    fn end(&mut self) {}
}

async fn scan(client: &dusk::Client) -> capnp::Result<Vec<u64>> {
    let program_args = Args::bind().as_program_args()?;
    let mut process_request = client.process_request();
    program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
    let process = process_request.send().promise.await?.get()?.get_result()?;
    let pid = process
        .pid_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result();
    let ran = process.run_request().send().promise;

    let keys = Rc::new(RefCell::new(Vec::new()));
    let scanned = async {
        let portal = process
            .portal_request()
            .send()
            .promise
            .await?
            .get()?
            .get_result()?
            .cast_to::<kvs_capnp::kvs_portal::Client>();
        let mut scan_request = portal.scan_request();
        scan_request
            .get()
            .set_output(capnp_rpc::new_client(Stream::new(ScannedKeys {
                keys: keys.clone(),
            })));
        scan_request.send().promise.await?;
        Ok::<(), capnp::Error>(())
    }
    .await;

    let mut kill_request = client.kill_request();
    kill_request.get().set_pid(pid);
    kill_request.get().set_signal(15);
    match kill_request.send().promise.await {
        Ok(_) => {
            if let Err(error) = ran.await {
                tracing::warn!(pid, error = %error, "the kvs process that scanned failed");
            }
            let mut waitpid_request = client.waitpid_request();
            waitpid_request.get().set_pid(pid);
            if let Err(error) = waitpid_request.send().promise.await {
                tracing::warn!(pid, error = %error, "couldn't reap the kvs process that scanned");
            }
        }
        Err(error) => {
            tracing::warn!(pid, error = %error, "couldn't stop the kvs process that scanned");
        }
    }
    scanned?;
    Ok(keys.take())
}

struct KvsProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for KvsProgramArgsBuilder {
    async fn build(&self, client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = KvsCli::try_parse_from(args)?;
        let args = match cli.action {
            KvsAction::Get { key } => {
                let typed = key_parse(&key);
                let scanned = match scan(&client).await {
                    Err(error) if error.kind == capnp::ErrorKind::Disconnected => {
                        tracing::warn!(key, error = %error, "scanning again for kvs get");
                        scan(&client).await
                    }
                    scanned => scanned,
                };
                let mut keys: Vec<u64> = match scanned {
                    Ok(scanned) => {
                        let keys: Vec<u64> = scanned
                            .into_iter()
                            .filter(|id| *id == typed || key_display(*id).starts_with(&key))
                            .collect();
                        anyhow::ensure!(!keys.is_empty(), "no key matches `{key}`");
                        keys
                    }
                    Err(error) if error.kind == capnp::ErrorKind::Disconnected => {
                        tracing::warn!(key, error = %error, "getting the key without scanning");
                        vec![typed]
                    }
                    Err(error) => return Err(error.into()),
                };
                keys.sort_by_cached_key(|id| key_display(*id));
                Args::get(&keys)
            }
            KvsAction::Set { key, value } => Args::set(key_parse(&key), &Value::String(value))?,
            KvsAction::Delete { key } => Args::delete(key_parse(&key)),
            KvsAction::Exists { key } => Args::exists(key_parse(&key)),
            KvsAction::Scan => Args::scan(),
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
* `kvs scan` lists every key: its name where the program that writes it
  registered one, and the id it travels as. A `<key>` anywhere above may be
  that id, as `0x…`, instead of a name.
* `kvs bind` runs no operation and leaves the process running, so a client can
  drive `get`, `set`, `delete` and `exists` over its portal instead. Stop it
  with `kill <pid>`.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(KvsProgramArgsBuilder {}),
    }
}
