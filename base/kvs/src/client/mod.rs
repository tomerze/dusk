use super::*;
use crate::kvs::key_id;
use crate::kvs_capnp::DEFAULT_PID;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program::stream::{Stream, StreamMixin};
use dusk_program_sh::client::cli::parse_pid;
use dusk_program_sh::client::client_hostname;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::borrow::ToOwned;
use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

mod bind;
mod resp;

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

fn pattern_matches(pattern: &str, name: &str) -> bool {
    let mut pieces = pattern.split('*');
    let Some(mut rest) = name.strip_prefix(pieces.next().unwrap_or_default()) else {
        return false;
    };
    for piece in pieces {
        let Some(index) = rest.find(piece) else {
            return false;
        };
        rest = &rest[index + piece.len()..];
    }
    true
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
    Set {
        key: String,
        value: String,
        #[arg(long)]
        sensitive: bool,
        #[arg(long)]
        persistent: bool,
        #[arg(long)]
        forbidden_unstick: bool,
    },
    /// Remove a key
    Delete {
        key: String,
        #[arg(long)]
        forbidden_unstick: bool,
    },
    /// Report whether a key is present
    Exists { key: String },
    #[command(about = "Serve the store to Redis clients on ADDRESS")]
    Bind {
        address: SocketAddr,
        #[arg(value_name = "PID", value_parser = parse_pid)]
        pid: Option<u64>,
        #[arg(long)]
        sensitive: bool,
        #[arg(long)]
        persistent: bool,
    },
    /// List every key, by name where a program registered one
    Scan,
    #[command(about = "Start the node's kvs server, or one at PID")]
    Server {
        #[arg(value_name = "PID", value_parser = parse_pid)]
        pid: Option<u64>,
    },
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
    let program_args = Args::server().as_program_args()?;
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
                            .filter(|id| *id == typed || pattern_matches(&key, &key_display(*id)))
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
            KvsAction::Set {
                key,
                value,
                sensitive,
                persistent,
                forbidden_unstick,
            } => {
                let mut flags = 0;
                if sensitive {
                    flags |= crate::kvs::FLAG_SENSITIVE;
                }
                if persistent {
                    flags |= crate::kvs::FLAG_PERSISTENT;
                }
                Args::set(
                    key_parse(&key),
                    &Value::String(value),
                    flags,
                    forbidden_unstick,
                )?
            }
            KvsAction::Delete {
                key,
                forbidden_unstick,
            } => Args::delete(key_parse(&key), forbidden_unstick),
            KvsAction::Exists { key } => Args::exists(key_parse(&key)),
            KvsAction::Scan => Args::scan(),
            KvsAction::Server { pid } => {
                let program_args = Args::server().as_program_args()?;
                program_args.set_pid(Some(pid.unwrap_or(DEFAULT_PID)))?;
                return Ok(program_args);
            }
            KvsAction::Bind {
                address,
                pid,
                sensitive,
                persistent,
            } => {
                let mut flags = 0;
                if sensitive {
                    flags |= crate::kvs::FLAG_SENSITIVE;
                }
                if persistent {
                    flags |= crate::kvs::FLAG_PERSISTENT;
                }
                Args::bind(
                    &client_hostname(),
                    dusk_capnp::capnp_rpc::new_client(bind::Bound {
                        client,
                        address,
                        server_pid: pid.unwrap_or(DEFAULT_PID),
                        flags,
                    }),
                )
            }
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
The key-value store is shared across all programs on the node, and held in
memory unless a key is persistent.

* `kvs get <key>` prints every key whose name starts with `<key>`, with its
  value, and fails if there is none. A `*` in `<key>` stands for any run of
  characters: `kvs get *` prints every key, and `kvs get dusk.*.uname` every
  key named `dusk.`, then anything, then `.uname`.
* `kvs set <key> <value>` stores `<value>` under `<key>`. Values typed at the
  prompt are stored as strings. It refuses a sticky key: one only Dusk sets,
  like `dusk.hostname`.
* `kvs set --forbidden-unstick <key> <value>` sets a sticky key anyway. The
  key holds the value until Dusk sets it again, and stays sticky.
* `kvs set --sensitive <key> <value>` marks the value a secret: `kvs` never
  writes it into a log or a trace. `kvs get` still prints it. A later `kvs set`
  without `--sensitive` clears the mark.
* `kvs set --persistent <key> <value>` keeps the key in the node's encrypted
  file as well, so it is still there after the node restarts. It fails on a
  node built without one. A later `kvs set` without `--persistent` takes the
  key out of the file.
* `kvs delete <key>` removes `<key>` and reports whether it was present. It
  refuses a sticky key; `kvs delete --forbidden-unstick <key>` removes it
  anyway.
* `kvs exists <key>` reports whether `<key>` is present.
* `kvs scan` lists every key: its name where the program that writes it
  registered one, the id it travels as, and its flags - `sticky` for a key
  only Dusk sets, `sensitive` for a secret, `persistent` for a key the node
  keeps in its file. A `<key>` anywhere above may be that id, as `0x…`,
  instead of a name.
* `kvs server [<pid>]` starts the node's default kvs server, or a kvs server
  at `<pid>`, and leaves it running, so a client can drive `get`, `set`,
  `delete`, `exists` and `scan` over its portal. Stop it with `kill <pid>`.
* `kvs bind <address> [<pid>]` listens on `<address>` (for example
  `127.0.0.1:6379`) on the client machine, speaking the Redis protocol, so
  `redis-cli -p 6379` reads and writes the store. Each Redis connection is
  served by the node's default kvs server, or by the one at `<pid>`, which the
  first connection starts if it is not running. Anything that reaches
  `<address>` can read and write every key: there is no password. A Redis
  write overrides a sticky key. `--sensitive` and `--persistent` mark every key
  the binding writes, as they do for `kvs set`. The command stays in the
  foreground until the process is terminated; `kill <pid>` from another client
  stops it, and so does Ctrl-C.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(KvsProgramArgsBuilder {}),
    }
}
