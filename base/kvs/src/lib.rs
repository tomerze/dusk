//! A key-value store, one per namespace, shared by every program on the node.
//!
//! Keys are named on the client and travel as ids ([`kvs::key_id`]); the node
//! holds no key strings.
//! Two names can hash to one id and silently share an entry.
#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

use alloc::rc::Rc;
use core::cell::{Cell, RefCell};

use dusk_program::{IntoCapnp, ready::Ready, signal::SignalReceiver};

extern crate alloc;
extern crate capnp;

#[cfg(feature = "client")]
pub mod client;
mod config;
pub use config::KvsConfig;
pub use dusk_program_kvs_internal as kvs;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("kvs", VERSION, kvs_capnp::PROGRAM_ID);

/// How many key ids travel in one value of a scan's stream.
const SCAN_PAGE_SIZE: usize = 64;

const CLIENT_FLAGS: u8 = kvs::FLAG_SENSITIVE;

fn flag_names(flags: u8) -> alloc::string::String {
    let mut names = alloc::vec::Vec::new();
    if flags & kvs::FLAG_STICKY != 0 {
        names.push("sticky");
    }
    if flags & kvs::FLAG_SENSITIVE != 0 {
        names.push("sensitive");
    }
    if flags & kvs::FLAG_PERSISTENT != 0 {
        names.push("persistent");
    }
    names.join(", ")
}

fn client_flags_refusal(flags: u8) -> Option<alloc::string::String> {
    (flags & !CLIENT_FLAGS != 0).then(|| {
        alloc::format!(
            "flags {flags:#04x} hold a flag a client cannot set; a client may set {}",
            flag_names(CLIENT_FLAGS)
        )
    })
}

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn get(keys: &[u64]) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut list = data.init_root().init_get(keys.len() as u32);
            for (index, key) in keys.iter().enumerate() {
                list.set(index as u32, *key);
            }
        }
        Args { data }
    }

    pub fn set(key: u64, value: &Value, flags: u8, forbidden_unstick: bool) -> capnp::Result<Self> {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            root.set_forbidden_unstick(forbidden_unstick);
            let mut set = root.init_set();
            set.set_key(key);
            set.set_flags(flags);
            value.write_to_builder(set.init_value())?;
        }
        Ok(Args { data })
    }

    pub fn delete(key: u64, forbidden_unstick: bool) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            root.set_forbidden_unstick(forbidden_unstick);
            root.set_delete(key);
        }
        Args { data }
    }

    pub fn exists(key: u64) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_exists(key);
        Args { data }
    }

    pub fn bind() -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_bind(());
        Args { data }
    }

    pub fn scan() -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_scan(());
        Args { data }
    }
}

#[cfg(not(feature = "client"))]
#[dusk_program_proc::impl_args_rpc_server]
impl Args {}

#[cfg(feature = "client")]
#[dusk_program_proc::impl_args_rpc_server]
impl Args {
    fn transpose(
        &mut self,
        params: kvs_capnp::kvs_args::server::TransposeParams,
        _results: kvs_capnp::kvs_args::server::TransposeResults,
    ) -> Promise<(), ::capnp::Error> {
        let parameters = dusk_capnp::pry!(params.get());
        let keys = dusk_capnp::pry!(parameters.get_keys());
        let output = dusk_capnp::pry!(parameters.get_output());

        let names = keys
            .iter()
            .map(|key| Value::String(client::key_display(key)))
            .collect();
        let column = if parameters.has_values() {
            let values = dusk_capnp::pry!(
                dusk_capnp::pry!(parameters.get_values())
                    .iter()
                    .map(Value::from_reader)
                    .collect::<::capnp::Result<Vec<_>>>()
            );
            (b"Value".to_vec(), Value::List(values))
        } else {
            (
                b"ID".to_vec(),
                Value::List(keys.iter().map(Value::Uint).collect()),
            )
        };
        let mut fields = alloc::vec![(b"Key".to_vec(), Value::List(names)), column];
        if parameters.has_flags() {
            let flags = dusk_capnp::pry!(parameters.get_flags());
            fields.push((
                b"Flags".to_vec(),
                Value::List(
                    flags
                        .iter()
                        .map(|flags| Value::String(flag_names(flags)))
                        .collect(),
                ),
            ));
        }
        let page = Record::with_fields(kvs_capnp::SCAN_TYPE_ID, fields);

        Promise::from_future(async move {
            let mut send_request = output.send_request();
            Value::Record(page).write_to_builder(send_request.get().init_value())?;
            send_request.send().await?;
            Ok(())
        })
    }
}

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher {
    persistent: Option<(u64, u64)>,
}

impl Launcher {
    pub fn new(config: KvsConfig) -> anyhow::Result<Self> {
        let persistent = match config.persistent {
            Some(path) => {
                let tid = dusk_core::driver::tid();
                Some((tid, kvs::register_persistent(tid, &path)?))
            }
            None => None,
        };
        Ok(Self { persistent })
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some((tid, generation)) = self.persistent {
            kvs::unregister_persistent(tid, generation);
        }
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

#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    /// What `output` streams; `None` streams nothing.
    result: Rc<RefCell<Option<Value>>>,
    found: Rc<RefCell<alloc::vec::Vec<(u64, Value)>>>,
    bound: Rc<Cell<bool>>,
    scanning: Rc<Cell<bool>>,
    kvs: alloc::sync::Arc<kvs::Kvs>,
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    pub async fn with_context(ctx: dusk_program::process::ProcessContext) -> anyhow::Result<Self> {
        let kvs = kvs::get_kvs(ctx.namespace.id);
        Ok(Process {
            result: Rc::new(RefCell::new(None)),
            found: Rc::new(RefCell::new(alloc::vec::Vec::new())),
            bound: Rc::new(Cell::new(false)),
            scanning: Rc::new(Cell::new(false)),
            kvs,
            ctx,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> portal::Client {
        let client: kvs_capnp::kvs_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        use kvs_capnp::kvs_args::data::Which;
        let (action, forbidden_unstick) = self
            .ctx
            .program_args
            .with_data::<kvs_capnp::kvs_args::data::Owned, _, _>(|data| {
                let action = match data.which()? {
                    Which::Get(keys) => Which::Get(keys?.iter().collect::<alloc::vec::Vec<_>>()),
                    Which::Set(set) => Which::Set((
                        set.get_key(),
                        Value::from_reader(set.get_value()?)?,
                        set.get_flags(),
                    )),
                    Which::Delete(key) => Which::Delete(key),
                    Which::Exists(key) => Which::Exists(key),
                    Which::Bind(()) => Which::Bind(()),
                    Which::Scan(()) => Which::Scan(()),
                };
                Ok((action, data.get_forbidden_unstick()))
            })?;
        match action {
            kvs_capnp::kvs_args::data::Which::Get(keys) => {
                anyhow::ensure!(!keys.is_empty(), "kvs get needs at least one key");
                let mut found = alloc::vec::Vec::new();
                for key in keys.iter().copied() {
                    if let Some(value) = self.kvs.get(key).await {
                        found.push((key, value));
                    }
                }
                if found.is_empty() {
                    let keys = keys
                        .iter()
                        .map(|key| alloc::format!("{key:#018x}"))
                        .collect::<alloc::vec::Vec<_>>()
                        .join(", ");
                    anyhow::bail!("key {keys} not found");
                }
                *self.found.borrow_mut() = found;
            }
            kvs_capnp::kvs_args::data::Which::Set((key, value, flags)) => {
                if let Some(refusal) = client_flags_refusal(flags) {
                    anyhow::bail!(refusal);
                }
                set_key(&self.kvs, key, value, flags, forbidden_unstick)
                    .await
                    .map_err(|error| {
                        unstick_hint(error, "`kvs set --forbidden-unstick` sets it anyway")
                    })?;
            }
            kvs_capnp::kvs_args::data::Which::Delete(key) => {
                let deleted = delete_key(&self.kvs, key, forbidden_unstick)
                    .await
                    .map_err(|error| {
                        unstick_hint(error, "`kvs delete --forbidden-unstick` deletes it anyway")
                    })?;
                *self.result.borrow_mut() = Some(Value::Bool(deleted));
            }
            kvs_capnp::kvs_args::data::Which::Exists(key) => {
                let exists = self.kvs.exists(key).await;
                *self.result.borrow_mut() = Some(Value::Bool(exists));
            }
            kvs_capnp::kvs_args::data::Which::Bind(()) => self.bound.set(true),
            kvs_capnp::kvs_args::data::Which::Scan(()) => self.scanning.set(true),
        }

        ready.sender().send(true);
        loop {
            let signal = signal_receiver.receive().await;
            if let Signal::Terminate = signal {
                return Ok(());
            }
        }
    }
}

async fn set_key(
    kvs: &kvs::Kvs,
    key: u64,
    value: Value,
    flags: u8,
    forbidden_unstick: bool,
) -> anyhow::Result<()> {
    if let Err(error) = kvs.set_unless_sticky(key, value.clone(), flags).await {
        if !forbidden_unstick || !error.is::<kvs::Sticky>() {
            return Err(error);
        }
        kvs.set(key, value, flags).await?;
        tracing::warn!(key, "forbidden-unstick overwrote a sticky key");
    }
    Ok(())
}

async fn delete_key(kvs: &kvs::Kvs, key: u64, forbidden_unstick: bool) -> anyhow::Result<bool> {
    match kvs.delete_unless_sticky(key).await {
        Err(error) if forbidden_unstick && error.is::<kvs::Sticky>() => {
            let deleted = kvs.delete(key).await?;
            if deleted {
                tracing::warn!(key, "forbidden-unstick removed a sticky key");
            }
            Ok(deleted)
        }
        deleted => deleted,
    }
}

fn unstick_hint(error: anyhow::Error, hint: &str) -> anyhow::Error {
    match error.downcast_ref::<kvs::Sticky>() {
        Some(sticky) => anyhow::anyhow!("{sticky}. {hint}"),
        None => error,
    }
}

#[derive(dusk_program_proc::Portal)]
pub struct Portal {
    pub process: Process,
}

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {
    fn get(
        &mut self,
        params: kvs_capnp::kvs_portal::GetParams,
        mut results: kvs_capnp::kvs_portal::GetResults,
    ) -> Promise<(), ::capnp::Error> {
        let key = dusk_capnp::pry!(params.get()).get_key();
        let kvs = self.process.kvs.clone();
        Promise::from_future(async move {
            let (value, flags) = kvs
                .get_with_flags(key)
                .await
                .ok_or_else(|| ::capnp::Error::failed(format!("key {key:#018x} not found")))?;
            value.write_to_builder(results.get().init_value())?;
            results.get().set_flags(flags);
            Ok(())
        })
    }

    fn set(
        &mut self,
        params: kvs_capnp::kvs_portal::SetParams,
        _results: kvs_capnp::kvs_portal::SetResults,
    ) -> Promise<(), ::capnp::Error> {
        let params = dusk_capnp::pry!(params.get());
        let key = params.get_key();
        let value = dusk_capnp::pry!(Value::from_reader(dusk_capnp::pry!(params.get_value())));
        let forbidden_unstick = params.get_forbidden_unstick();
        let flags = params.get_flags();
        if let Some(refusal) = client_flags_refusal(flags) {
            return Promise::err(::capnp::Error::failed(refusal));
        }
        let kvs = self.process.kvs.clone();
        Promise::from_future(async move {
            set_key(&kvs, key, value, flags, forbidden_unstick)
                .await
                .map_err(|error| unstick_hint(error, "`forbiddenUnstick` sets it anyway"))
                .into_capnp()
        })
    }

    fn delete(
        &mut self,
        params: kvs_capnp::kvs_portal::DeleteParams,
        mut results: kvs_capnp::kvs_portal::DeleteResults,
    ) -> Promise<(), ::capnp::Error> {
        let params = dusk_capnp::pry!(params.get());
        let key = params.get_key();
        let forbidden_unstick = params.get_forbidden_unstick();
        let kvs = self.process.kvs.clone();
        Promise::from_future(async move {
            let deleted = delete_key(&kvs, key, forbidden_unstick)
                .await
                .map_err(|error| unstick_hint(error, "`forbiddenUnstick` deletes it anyway"))
                .into_capnp()?;
            results.get().set_deleted(deleted);
            Ok(())
        })
    }

    fn exists(
        &mut self,
        params: kvs_capnp::kvs_portal::ExistsParams,
        mut results: kvs_capnp::kvs_portal::ExistsResults,
    ) -> Promise<(), ::capnp::Error> {
        let key = dusk_capnp::pry!(params.get()).get_key();
        let kvs = self.process.kvs.clone();
        Promise::from_future(async move {
            results.get().set_exists(kvs.exists(key).await);
            Ok(())
        })
    }

    fn scan(
        &mut self,
        params: kvs_capnp::kvs_portal::ScanParams,
        _results: kvs_capnp::kvs_portal::ScanResults,
    ) -> Promise<(), ::capnp::Error> {
        let output = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_output());
        let kvs = self.process.kvs.clone();
        Promise::from_future(async move {
            let keys = kvs.scan().await;
            for page in keys.chunks(SCAN_PAGE_SIZE) {
                let mut send_request = output.send_request();
                Value::List(page.iter().map(|(key, _)| Value::Uint(*key)).collect())
                    .write_to_builder(send_request.get().init_value())?;
                send_request.send().await?;
            }
            output.done_request().send().promise.await?;
            Ok(())
        })
    }
}

impl dusk_program_sh::sh_capnp::output_portal::Server for Portal {
    fn output(
        &mut self,
        params: dusk_program_sh::sh_capnp::output_portal::OutputParams,
        mut results: dusk_program_sh::sh_capnp::output_portal::OutputResults,
    ) -> Promise<(), ::capnp::Error> {
        dusk_capnp::pry!(results.set_pipeline());
        let stream = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_stream());
        if self.process.bound.get() {
            results.get().set_daemonize(true);
            return Promise::ok(());
        }
        let result = self.process.result.borrow_mut().take();
        let found = core::mem::take(&mut *self.process.found.borrow_mut());
        let process = self.process.clone();
        Promise::from_future(async move {
            let rows = if process.scanning.get() {
                let (keys, flags): (alloc::vec::Vec<u64>, alloc::vec::Vec<u8>) =
                    process.kvs.scan().await.into_iter().unzip();
                Some((keys, None, Some(flags)))
            } else if found.is_empty() {
                None
            } else {
                let (keys, values): (alloc::vec::Vec<u64>, alloc::vec::Vec<Value>) =
                    found.into_iter().unzip();
                Some((keys, Some(values), None))
            };
            if let Some((keys, values, flags)) = rows {
                let untransposed = |error: ::capnp::Error| {
                    if !matches!(
                        error.kind,
                        ::capnp::ErrorKind::MessageContainsNullCapabilityPointer
                            | ::capnp::ErrorKind::Disconnected
                            | ::capnp::ErrorKind::PrematureEndOfFile
                    ) {
                        return Err(error);
                    }
                    tracing::warn!(
                        pid = process.ctx.pid,
                        error = %error,
                        "sending keys without their names"
                    );
                    Ok(())
                };
                let mut server = match process
                    .ctx
                    .program_args
                    .server_as::<kvs_capnp::kvs_args::server::Client>()
                {
                    Ok(server) => Some(server),
                    Err(error) => {
                        untransposed(error)?;
                        None
                    }
                };
                let mut value_pages = values
                    .as_deref()
                    .map(|values| values.chunks(SCAN_PAGE_SIZE));
                let mut flag_pages = flags.as_deref().map(|flags| flags.chunks(SCAN_PAGE_SIZE));
                for key_page in keys.chunks(SCAN_PAGE_SIZE) {
                    let value_page = value_pages.as_mut().and_then(Iterator::next);
                    let flag_page = flag_pages.as_mut().and_then(Iterator::next);
                    if let Some(server) = &server {
                        let mut request = server.transpose_request();
                        {
                            let mut builder = request.get();
                            builder.set_output(stream.clone());
                            if let Some(value_page) = value_page {
                                let mut values =
                                    builder.reborrow().init_values(value_page.len() as u32);
                                for (index, value) in value_page.iter().enumerate() {
                                    value.write_to_builder(values.reborrow().get(index as u32))?;
                                }
                            }
                            if let Some(flag_page) = flag_page {
                                let mut flags =
                                    builder.reborrow().init_flags(flag_page.len() as u32);
                                for (index, entry_flags) in flag_page.iter().enumerate() {
                                    flags.set(index as u32, *entry_flags);
                                }
                            }
                            let mut keys = builder.init_keys(key_page.len() as u32);
                            for (index, key) in key_page.iter().enumerate() {
                                keys.set(index as u32, *key);
                            }
                        }
                        match request.send().promise.await {
                            Ok(_) => continue,
                            Err(error) => untransposed(error)?,
                        }
                    }
                    server = None;
                    let mut fields = alloc::vec![(
                        b"ID".to_vec(),
                        Value::List(key_page.iter().copied().map(Value::Uint).collect()),
                    )];
                    if let Some(value_page) = value_page {
                        fields.push((b"Value".to_vec(), Value::List(value_page.to_vec())));
                    }
                    if let Some(flag_page) = flag_page {
                        fields.push((
                            b"Flags".to_vec(),
                            Value::List(
                                flag_page
                                    .iter()
                                    .map(|flags| Value::String(flag_names(*flags)))
                                    .collect(),
                            ),
                        ));
                    }
                    let mut send_request = stream.send_request();
                    Value::Record(Record::with_fields(kvs_capnp::SCAN_TYPE_ID, fields))
                        .write_to_builder(send_request.get().init_value())?;
                    send_request.send().await?;
                }
                results.get().set_daemonize(false);
                return Ok(());
            }
            if let Some(value) = result {
                let mut send_request = stream.send_request();
                let value_builder = send_request.get().init_value();
                value.write_to_builder(value_builder)?;
                send_request.send().await?;
            }
            results.get().set_daemonize(false);
            Ok(())
        })
    }
}
