use super::*;
use clap::Parser as _;
use dusk_program::anyhow::Context as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::io::{Read as _, SeekFrom};
use std::rc::Rc;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

#[derive(clap::Parser)]
#[command(name = "cp", no_binary_name = true)]
struct CpCli {
    #[arg(help = "The file to copy; a leading `:` names a file on the node")]
    source: String,
    #[arg(help = "Where to copy it; a leading `:` names a file on the node")]
    destination: String,
}

fn set_location(mut builder: cp_capnp::location::Builder, path: &str) {
    match path.strip_prefix(':') {
        Some(path) => builder.set_node(path),
        None => builder.set_client(path),
    }
}

enum Side {
    Source,
    Destination,
}

impl Args {
    pub fn new(source: &str, destination: &str) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            set_location(root.reborrow().init_source(), source);
            set_location(root.init_destination(), destination);
        }
        Args { data }
    }

    fn ensure_given(&self, action: &str, path: &str, sides: &[Side]) -> Result<(), ::capnp::Error> {
        let data = self.data.get_root_as_reader()?;
        for side in sides {
            let location = match side {
                Side::Source => data.get_source()?,
                Side::Destination => data.get_destination()?,
            };
            if let cp_capnp::location::Which::Client(own) = location.which()?
                && own?.to_str()? == path
            {
                return Ok(());
            }
        }
        tracing::warn!(
            action,
            path,
            "refused a node that asked for a path this cp was not given"
        );
        Err(::capnp::Error::failed(format!(
            "refused to {action} `{path}`: this cp was not given that path"
        )))
    }
}

fn hash_file(path: &str, length: u64) -> anyhow::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path).with_context(|| format!("couldn't open `{path}`"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; CHUNK_LENGTH];
    let mut offset = 0;
    while offset < length {
        let wanted = (length - offset).min(CHUNK_LENGTH as u64) as usize;
        let count = file
            .read(&mut buffer[..wanted])
            .with_context(|| format!("couldn't read `{path}` at {offset}"))?;
        if count == 0 {
            bail!("`{path}` ended at byte {offset} of {length}");
        }
        hasher.update(&buffer[..count]);
        offset += count as u64;
    }
    Ok(hasher.finalize().into())
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {
    fn stat(
        &mut self,
        params: cp_capnp::cp_args::server::StatParams,
        mut results: cp_capnp::cp_args::server::StatResults,
    ) -> Promise<(), ::capnp::Error> {
        let path = dusk_capnp::pry!(
            dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_path()).to_string()
        );
        dusk_capnp::pry!(self.ensure_given("stat", &path, &[Side::Source, Side::Destination]));
        match std::fs::metadata(&path) {
            Ok(metadata) => {
                results.get().set_exists(true);
                results.get().set_length(metadata.len());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                results.get().set_exists(false);
            }
            Err(error) => {
                return Promise::err(::capnp::Error::failed(format!(
                    "couldn't stat `{path}`: {error}"
                )));
            }
        }
        Promise::ok(())
    }

    fn hash(
        &mut self,
        params: cp_capnp::cp_args::server::HashParams,
        mut results: cp_capnp::cp_args::server::HashResults,
    ) -> Promise<(), ::capnp::Error> {
        let params = dusk_capnp::pry!(params.get());
        let path = dusk_capnp::pry!(dusk_capnp::pry!(params.get_path()).to_string());
        dusk_capnp::pry!(self.ensure_given("hash", &path, &[Side::Source, Side::Destination]));
        let length = params.get_length();
        Promise::from_future(async move {
            let hash = tokio::task::spawn_blocking(move || hash_file(&path, length))
                .await
                .map_err(|error| ::capnp::Error::failed(error.to_string()))?
                .into_capnp()?;
            results.get().set_hash(&hash);
            Ok(())
        })
    }

    fn read(
        &mut self,
        params: cp_capnp::cp_args::server::ReadParams,
        _results: cp_capnp::cp_args::server::ReadResults,
    ) -> Promise<(), ::capnp::Error> {
        let params = dusk_capnp::pry!(params.get());
        let path = dusk_capnp::pry!(dusk_capnp::pry!(params.get_path()).to_string());
        dusk_capnp::pry!(self.ensure_given("read", &path, &[Side::Source]));
        let offset = params.get_offset();
        let end = params.get_end();
        let sink = dusk_capnp::pry!(params.get_sink());
        Promise::from_future(async move {
            let sent: anyhow::Result<()> = async {
                let mut file = tokio::fs::File::open(&path)
                    .await
                    .with_context(|| format!("couldn't open `{path}`"))?;
                file.seek(SeekFrom::Start(offset))
                    .await
                    .with_context(|| format!("couldn't seek `{path}` to {offset}"))?;
                let mut buffer = vec![0; CHUNK_LENGTH];
                let mut position = offset;
                while position < end {
                    let wanted = (end - position).min(CHUNK_LENGTH as u64) as usize;
                    let count = file
                        .read(&mut buffer[..wanted])
                        .await
                        .with_context(|| format!("couldn't read `{path}` at {position}"))?;
                    if count == 0 {
                        bail!("`{path}` ended at byte {position} of {end}");
                    }
                    let mut request = sink.write_request();
                    request.get().set_offset(position);
                    request.get().set_bytes(&buffer[..count]);
                    request.send().await?;
                    position += count as u64;
                }
                let mut request = sink.done_request();
                request.get().set_end(end);
                request.send().promise.await?;
                Ok(())
            }
            .await;
            sent.into_capnp()
        })
    }

    fn write(
        &mut self,
        params: cp_capnp::cp_args::server::WriteParams,
        mut results: cp_capnp::cp_args::server::WriteResults,
    ) -> Promise<(), ::capnp::Error> {
        let params = dusk_capnp::pry!(params.get());
        let path = dusk_capnp::pry!(dusk_capnp::pry!(params.get_path()).to_string());
        dusk_capnp::pry!(self.ensure_given("write", &path, &[Side::Destination]));
        let offset = params.get_offset();
        Promise::from_future(async move {
            let file: anyhow::Result<tokio::fs::File> = async {
                let file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&path)
                    .await
                    .with_context(|| format!("couldn't open `{path}`"))?;
                file.set_len(offset)
                    .await
                    .with_context(|| format!("couldn't truncate `{path}` to {offset}"))?;
                Ok(file)
            }
            .await;
            let sink: cp_capnp::sink::Client = capnp_rpc::new_client(ClientSink {
                file: Rc::new(tokio::sync::Mutex::new(file.into_capnp()?)),
                path,
                start: offset,
                written: Rc::new(Cell::new(0)),
                failed: Rc::new(Cell::new(false)),
                changed: Rc::new(tokio::sync::Notify::new()),
            });
            results.get().set_sink(sink);
            Ok(())
        })
    }
}

struct ClientSink {
    file: Rc<tokio::sync::Mutex<tokio::fs::File>>,
    path: String,
    start: u64,
    written: Rc<Cell<u64>>,
    failed: Rc<Cell<bool>>,
    changed: Rc<tokio::sync::Notify>,
}

impl cp_capnp::sink::Server for ClientSink {
    fn write(&mut self, params: cp_capnp::sink::WriteParams) -> Promise<(), ::capnp::Error> {
        let file = self.file.clone();
        let path = self.path.clone();
        let written = self.written.clone();
        let failed = self.failed.clone();
        let changed = self.changed.clone();
        Promise::from_future(async move {
            let result: anyhow::Result<()> = async {
                let params = params.get()?;
                let offset = params.get_offset();
                let bytes = params.get_bytes()?;
                let mut file = file.lock().await;
                file.seek(SeekFrom::Start(offset))
                    .await
                    .and(file.write_all(bytes).await)
                    .with_context(|| format!("couldn't write `{path}` at {offset}"))?;
                written.set(written.get() + bytes.len() as u64);
                Ok(())
            }
            .await;
            if result.is_err() {
                failed.set(true);
            }
            changed.notify_one();
            result.into_capnp()
        })
    }

    fn done(
        &mut self,
        params: cp_capnp::sink::DoneParams,
        _results: cp_capnp::sink::DoneResults,
    ) -> Promise<(), ::capnp::Error> {
        let end = dusk_capnp::pry!(params.get()).get_end();
        let file = self.file.clone();
        let path = self.path.clone();
        let start = self.start;
        let written = self.written.clone();
        let failed = self.failed.clone();
        let changed = self.changed.clone();
        Promise::from_future(async move {
            let result: anyhow::Result<()> = async {
                loop {
                    if failed.get() {
                        bail!("a write to `{path}` failed");
                    }
                    let reached = start + written.get();
                    if reached == end {
                        break;
                    }
                    if reached > end {
                        bail!("`{path}` was sent {reached} bytes, past the end at {end}");
                    }
                    changed.notified().await;
                }
                let mut file = file.lock().await;
                file.flush()
                    .await
                    .with_context(|| format!("couldn't flush `{path}`"))?;
                file.sync_all()
                    .await
                    .with_context(|| format!("couldn't sync `{path}`"))
            }
            .await;
            result.into_capnp()
        })
    }
}

struct CpProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for CpProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = CpCli::try_parse_from(args)?;
        Ok(Args::new(&cli.source, &cli.destination).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(cp_capnp::PROGRAM_ID),
            name: "cp",
            short_description: "copy a file between the client and the node",
            long_description: r#"
The `cp` program copies one file. A path that starts with `:` is on the node;
any other path is on the client that runs the command.

* `cp report.txt :/tmp/report.txt` uploads `report.txt` to the node.
* `cp :/var/log/node.log node.log` downloads `/var/log/node.log` from the node.
* `cp :/a :/b` copies a file on the node; `cp a b` copies a file on the client.

If the destination already holds the start of the source - a copy that was cut
off - `cp` checks that part by its SHA-256 and sends only the rest. Otherwise it
overwrites the destination. When the copy is done, `cp` compares the SHA-256 of
the destination with the source's and fails if they differ.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(CpProgramArgsBuilder {}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(std::path::PathBuf);

    impl Directory {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("dusk-cp-client-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Directory(path)
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).display().to_string()
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                std::eprintln!("couldn't remove {}: {error}", self.0.display());
            }
        }
    }

    fn server(source: &str, destination: &str) -> cp_capnp::cp_args::server::Client {
        Args::new(source, destination)
            .as_program_args()
            .unwrap()
            .server_as()
            .unwrap()
    }

    async fn stat(
        server: &cp_capnp::cp_args::server::Client,
        path: &str,
    ) -> Result<bool, ::capnp::Error> {
        let mut request = server.stat_request();
        request.get().set_path(path);
        Ok(request.send().promise.await?.get()?.get_exists())
    }

    async fn hash(
        server: &cp_capnp::cp_args::server::Client,
        path: &str,
    ) -> Result<Vec<u8>, ::capnp::Error> {
        let mut request = server.hash_request();
        request.get().set_path(path);
        request.get().set_length(1);
        Ok(request.send().promise.await?.get()?.get_hash()?.to_vec())
    }

    async fn read(
        server: &cp_capnp::cp_args::server::Client,
        path: &str,
    ) -> Result<(), ::capnp::Error> {
        let mut request = server.read_request();
        request.get().set_path(path);
        request.get().set_end(1);
        request.send().promise.await?;
        Ok(())
    }

    async fn write(
        server: &cp_capnp::cp_args::server::Client,
        path: &str,
    ) -> Result<(), ::capnp::Error> {
        let mut request = server.write_request();
        request.get().set_path(path);
        request.send().promise.await?.get()?.get_sink()?;
        Ok(())
    }

    fn refused<Answer: std::fmt::Debug>(answer: Result<Answer, ::capnp::Error>, action: &str) {
        let error = answer.expect_err("the client answered for a path it was not given");
        assert!(
            error.to_string().contains(&format!("refused to {action}")),
            "{error}"
        );
    }

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    #[test]
    fn test_an_upload_answers_only_for_its_own_source() {
        let directory = Directory::new("upload");
        let source = directory.path("source");
        let secret = directory.path("secret");
        std::fs::write(&source, "copied").unwrap();
        std::fs::write(&secret, "secret").unwrap();
        run(async {
            let server = server(&source, ":/tmp/destination");
            assert!(stat(&server, &source).await.unwrap());
            assert_eq!(hash(&server, &source).await.unwrap().len(), 32);
            refused(stat(&server, &secret).await, "stat");
            refused(hash(&server, &secret).await, "hash");
            refused(read(&server, &secret).await, "read");
            refused(write(&server, &secret).await, "write");
            refused(write(&server, &source).await, "write");
            refused(stat(&server, "/tmp/destination").await, "stat");
        });
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "copied");
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "secret");
    }

    #[test]
    fn test_a_download_answers_only_for_its_own_destination() {
        let directory = Directory::new("download");
        let destination = directory.path("destination");
        let elsewhere = directory.path("elsewhere");
        run(async {
            let server = server(":/etc/hostname", &destination);
            assert!(!stat(&server, &destination).await.unwrap());
            write(&server, &destination).await.unwrap();
            assert!(stat(&server, &destination).await.unwrap());
            refused(read(&server, &destination).await, "read");
            refused(write(&server, &elsewhere).await, "write");
            refused(stat(&server, "/etc/hostname").await, "stat");
        });
        assert!(std::path::Path::new(&destination).exists());
        assert!(!std::path::Path::new(&elsewhere).exists());
    }
}
