#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

use alloc::rc::Rc;
use core::cell::{Cell, RefCell};
use core::pin::pin;

use dusk_core::driver::{File, OpenMode};
use dusk_program::anyhow::{anyhow, bail};
use dusk_program::embassy_futures::join::join;
use dusk_program::embassy_futures::select::{Either, select};
use dusk_program::embassy_futures::yield_now;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::{ready::Ready, signal::SignalReceiver};
use sha2::{Digest, Sha256};

extern crate alloc;
extern crate capnp;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("cp", VERSION, cp_capnp::PROGRAM_ID);

#[cfg(feature = "client")]
pub mod client;

const CHUNK_LENGTH: usize = 1 << 20;

type Changed = dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, ()>;

#[cfg(feature = "client")]
#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

#[derive(dusk_program_proc::Launcher, Default)]
pub struct Launcher;

impl Launcher {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::launcher::LauncherMixin for Launcher {
    async fn launch(
        &mut self,
        process_context: ProcessContext,
    ) -> anyhow::Result<Box<dyn dusk_program::process::Process>> {
        Ok(Box::new(Process::with_context(process_context).await?))
    }
}

enum Location {
    Node(alloc::string::String),
    Client(alloc::string::String),
}

impl Location {
    fn from_reader(reader: cp_capnp::location::Reader) -> capnp::Result<Self> {
        Ok(match reader.which()? {
            cp_capnp::location::Which::Node(path) => Location::Node(path?.to_string()?),
            cp_capnp::location::Which::Client(path) => Location::Client(path?.to_string()?),
        })
    }
}

enum Endpoint {
    Node {
        name: alloc::string::String,
        file: Rc<dyn File>,
    },
    Client {
        name: alloc::string::String,
        server: cp_capnp::cp_args::server::Client,
    },
}

impl Endpoint {
    async fn open(
        ctx: &ProcessContext,
        location: &Location,
        destination: bool,
    ) -> anyhow::Result<Self> {
        match location {
            Location::Node(path) => {
                let mode = OpenMode {
                    read: true,
                    write: destination,
                    create: destination,
                    truncate: false,
                };
                let file = dusk_core::driver::fs_driver()?.open(path, mode).await?;
                Ok(Endpoint::Node {
                    name: alloc::format!(":{path}"),
                    file: Rc::from(file),
                })
            }
            Location::Client(path) => {
                let server = ctx.program_args.server_as().map_err(|error| {
                    if let ::capnp::ErrorKind::MessageContainsNullCapabilityPointer = error.kind {
                        anyhow!("`{path}` is on the client, and this cp has no client to reach it")
                    } else {
                        anyhow!(error)
                    }
                })?;
                Ok(Endpoint::Client {
                    name: path.clone(),
                    server,
                })
            }
        }
    }

    fn name(&self) -> &str {
        match self {
            Endpoint::Node { name, .. } | Endpoint::Client { name, .. } => name,
        }
    }

    async fn length(&self) -> anyhow::Result<Option<u64>> {
        match self {
            Endpoint::Node { file, .. } => Ok(Some(file.stat().await?.length)),
            Endpoint::Client { name, server } => {
                let mut request = server.stat_request();
                request.get().set_path(name.as_str());
                let response = request.send().promise.await?;
                let results = response.get()?;
                Ok(results.get_exists().then(|| results.get_length()))
            }
        }
    }

    async fn hash(&self, length: u64) -> anyhow::Result<[u8; 32]> {
        match self {
            Endpoint::Node { name, file } => {
                let mut hasher = Sha256::new();
                let mut buffer = alloc::vec![0; CHUNK_LENGTH];
                let mut offset = 0;
                while offset < length {
                    let wanted = (length - offset).min(CHUNK_LENGTH as u64) as usize;
                    let count = file.read(offset, &mut buffer[..wanted]).await?;
                    if count == 0 {
                        bail!("`{name}` ended at byte {offset} of {length}");
                    }
                    hasher.update(&buffer[..count]);
                    offset += count as u64;
                    yield_now().await;
                }
                Ok(hasher.finalize().into())
            }
            Endpoint::Client { name, server } => {
                let mut request = server.hash_request();
                request.get().set_path(name.as_str());
                request.get().set_length(length);
                let response = request.send().promise.await?;
                let hash = response.get()?.get_hash()?;
                hash.try_into().map_err(|_| {
                    anyhow!(
                        "the client answered a {}-byte SHA-256 for `{name}`",
                        hash.len()
                    )
                })
            }
        }
    }

    async fn sink(&self, offset: u64) -> anyhow::Result<cp_capnp::sink::Client> {
        match self {
            Endpoint::Node { name, file } => {
                file.truncate(offset).await?;
                Ok(capnp_rpc::new_client(FileSink {
                    name: name.clone(),
                    file: file.clone(),
                    start: offset,
                    written: Rc::new(Cell::new(0)),
                    failed: Rc::new(Cell::new(false)),
                    changed: Rc::new(Changed::new()),
                }))
            }
            Endpoint::Client { name, server } => {
                let mut request = server.write_request();
                request.get().set_path(name.as_str());
                request.get().set_offset(offset);
                let response = request.send().promise.await?;
                Ok(response.get()?.get_sink()?)
            }
        }
    }

    async fn read(
        &self,
        offset: u64,
        end: u64,
        sink: cp_capnp::sink::Client,
    ) -> anyhow::Result<()> {
        match self {
            Endpoint::Node { name, file } => {
                let mut buffer = alloc::vec![0; CHUNK_LENGTH];
                let mut position = offset;
                while position < end {
                    let wanted = (end - position).min(CHUNK_LENGTH as u64) as usize;
                    let count = file.read(position, &mut buffer[..wanted]).await?;
                    if count == 0 {
                        bail!("`{name}` ended at byte {position} of {end}");
                    }
                    let mut request = sink.write_request();
                    request.get().set_offset(position);
                    request.get().set_bytes(&buffer[..count]);
                    request.send().await?;
                    position += count as u64;
                    yield_now().await;
                }
                let mut request = sink.done_request();
                request.get().set_end(end);
                request.send().promise.await?;
                Ok(())
            }
            Endpoint::Client { name, server } => {
                let mut request = server.read_request();
                request.get().set_path(name.as_str());
                request.get().set_offset(offset);
                request.get().set_end(end);
                request.get().set_sink(sink);
                request.send().promise.await?;
                Ok(())
            }
        }
    }
}

struct Copied {
    source: alloc::string::String,
    destination: alloc::string::String,
    length: u64,
    resumed: u64,
    hash: [u8; 32],
}

async fn copy(
    ctx: &ProcessContext,
    source: &Location,
    destination: &Location,
) -> anyhow::Result<Copied> {
    let source = Endpoint::open(ctx, source, false).await?;
    let length = source
        .length()
        .await?
        .ok_or_else(|| anyhow!("`{}` does not exist", source.name()))?;
    let destination = Endpoint::open(ctx, destination, true).await?;
    let existing = destination.length().await?.unwrap_or(0);
    let resumed = if existing > 0 && existing <= length {
        let (source_hash, destination_hash) =
            join(source.hash(existing), destination.hash(existing)).await;
        if source_hash? == destination_hash? {
            existing
        } else {
            0
        }
    } else {
        0
    };
    tracing::info!(
        source = source.name(),
        destination = destination.name(),
        length,
        existing,
        resumed,
        "copying"
    );

    let sink = destination.sink(resumed).await?;
    source.read(resumed, length, sink).await?;

    let (source_hash, destination_hash) = join(source.hash(length), destination.hash(length)).await;
    let (source_hash, destination_hash) = (source_hash?, destination_hash?);
    let copied_length = destination.length().await?.unwrap_or(0);
    if copied_length != length {
        bail!(
            "`{}` is {copied_length} bytes after the copy, and `{}` is {length}",
            destination.name(),
            source.name()
        );
    }
    if source_hash != destination_hash {
        bail!(
            "the SHA-256 of `{}` ({}) does not match the SHA-256 of `{}` ({})",
            destination.name(),
            hex(&destination_hash),
            source.name(),
            hex(&source_hash)
        );
    }
    tracing::info!(
        source = source.name(),
        destination = destination.name(),
        length,
        resumed,
        "copied"
    );
    Ok(Copied {
        source: source.name().into(),
        destination: destination.name().into(),
        length,
        resumed,
        hash: source_hash,
    })
}

fn hex(bytes: &[u8]) -> alloc::string::String {
    bytes
        .iter()
        .map(|byte| alloc::format!("{byte:02x}"))
        .collect()
}

struct FileSink {
    name: alloc::string::String,
    file: Rc<dyn File>,
    start: u64,
    written: Rc<Cell<u64>>,
    failed: Rc<Cell<bool>>,
    changed: Rc<Changed>,
}

impl cp_capnp::sink::Server for FileSink {
    fn write(&mut self, params: cp_capnp::sink::WriteParams) -> Promise<(), ::capnp::Error> {
        let file = self.file.clone();
        let name = self.name.clone();
        let written = self.written.clone();
        let failed = self.failed.clone();
        let changed = self.changed.clone();
        Promise::from_future(async move {
            let result: anyhow::Result<()> = async {
                let params = params.get()?;
                let mut offset = params.get_offset();
                let mut bytes = params.get_bytes()?;
                while !bytes.is_empty() {
                    let count = file.write(offset, bytes).await?;
                    if count == 0 {
                        bail!("`{name}` took no bytes at {offset}");
                    }
                    offset += count as u64;
                    bytes = &bytes[count..];
                    written.set(written.get() + count as u64);
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                failed.set(true);
            }
            changed.signal(());
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
        let name = self.name.clone();
        let start = self.start;
        let written = self.written.clone();
        let failed = self.failed.clone();
        let changed = self.changed.clone();
        Promise::from_future(async move {
            let result: anyhow::Result<()> = async {
                loop {
                    if failed.get() {
                        bail!("a write to `{name}` failed");
                    }
                    let reached = start + written.get();
                    if reached == end {
                        break;
                    }
                    if reached > end {
                        bail!("`{name}` was sent {reached} bytes, past the end at {end}");
                    }
                    changed.wait().await;
                }
                file.sync().await
            }
            .await;
            result.into_capnp()
        })
    }
}

#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    copied: Rc<RefCell<Option<Copied>>>,
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    pub async fn with_context(ctx: dusk_program::process::ProcessContext) -> anyhow::Result<Self> {
        Ok(Process {
            copied: Rc::new(RefCell::new(None)),
            ctx,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> portal::Client {
        let client: cp_capnp::cp_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        let (source, destination) = self
            .ctx
            .program_args
            .with_data::<cp_capnp::cp_args::data::Owned, _, _>(|data| {
                Ok((
                    Location::from_reader(data.get_source()?)?,
                    Location::from_reader(data.get_destination()?)?,
                ))
            })?;

        let copying = pin!(copy(&self.ctx, &source, &destination));
        let terminated = pin!(async {
            loop {
                if let Signal::Terminate = signal_receiver.receive().await {
                    return;
                }
            }
        });
        match select(copying, terminated).await {
            Either::First(copied) => *self.copied.borrow_mut() = Some(copied?),
            Either::Second(()) => {
                tracing::info!(pid = self.ctx.pid, "copy terminated before it finished");
                return Ok(());
            }
        }

        ready.sender().send(true);
        loop {
            if let Signal::Terminate = signal_receiver.receive().await {
                return Ok(());
            }
        }
    }
}

#[derive(dusk_program_proc::Portal)]
pub struct Portal {
    pub process: Process,
}

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {}

impl dusk_program_sh::sh_capnp::output_portal::Server for Portal {
    fn output(
        &mut self,
        params: dusk_program_sh::sh_capnp::output_portal::OutputParams,
        mut results: dusk_program_sh::sh_capnp::output_portal::OutputResults,
    ) -> Promise<(), ::capnp::Error> {
        dusk_capnp::pry!(results.set_pipeline());
        let stream = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_stream());
        let copied = self.process.copied.borrow_mut().take();
        Promise::from_future(async move {
            if let Some(copied) = copied {
                let record = Record::with_fields(
                    cp_capnp::RESULT_TYPE_ID,
                    [
                        (b"source".to_vec(), Value::String(copied.source)),
                        (b"destination".to_vec(), Value::String(copied.destination)),
                        (b"length".to_vec(), Value::Uint(copied.length)),
                        (b"resumed".to_vec(), Value::Uint(copied.resumed)),
                        (b"sha256".to_vec(), Value::String(hex(&copied.hash))),
                    ],
                );
                let mut request = stream.send_request();
                Value::Record(record).write_to_builder(request.get().init_value())?;
                request.send().await?;
            }
            results.get().set_daemonize(false);
            Ok(())
        })
    }
}
