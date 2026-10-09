#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

use dusk_capnp::capnp::message::HeapAllocator;
use dusk_capnp::capnp_rpc::ImbuedMessageBuilder;
use dusk_program::{ready::Ready, signal::SignalReceiver};
use dusk_program_sh::{BytecodeMessage, bytecode};

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("init", VERSION, init_capnp::PROGRAM_ID);

// Hashed in const context, so the node carries the ids and never the names.
const VERSION_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.version");
const GIT_REV_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.git_rev");
const NAMESPACE_ID_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.namespace_id");
const TID_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.tid");
const HOSTNAME_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.hostname");
const ARCH_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.target.arch");
const OS_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.target.os");
const BITS_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.target.bits");
const IMPL_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.impl");

const DUSK_KEY_NAMES: [&str; 71] = [
    "dusk.version",
    "dusk.git_rev",
    "dusk.namespace_id",
    "dusk.tid",
    "dusk.hostname",
    "dusk.target.arch",
    "dusk.target.os",
    "dusk.target.bits",
    "dusk.impl",
    "dusk.os.process.pid",
    "dusk.os.process.executable",
    "dusk.os.process.working_directory",
    "dusk.os.process.parent_pid",
    "dusk.os.time_zone",
    "dusk.os.locale",
    "dusk.os.nix.uname.sysname",
    "dusk.os.nix.uname.nodename",
    "dusk.os.nix.uname.release",
    "dusk.os.nix.uname.version",
    "dusk.os.nix.uname.machine",
    "dusk.os.nix.uname.domainname",
    "dusk.os.nix.uid",
    "dusk.os.nix.euid",
    "dusk.os.nix.limits.open_files.soft",
    "dusk.os.nix.limits.open_files.hard",
    "dusk.os.nix.limits.core_file_size.soft",
    "dusk.os.nix.limits.core_file_size.hard",
    "dusk.os.linux.boot_id",
    "dusk.os.linux.pid1",
    "dusk.os.linux.glibc_version",
    "dusk.os.linux.os_release.id",
    "dusk.os.linux.os_release.id_like",
    "dusk.os.linux.os_release.name",
    "dusk.os.linux.os_release.pretty_name",
    "dusk.os.linux.os_release.version",
    "dusk.os.linux.os_release.version_codename",
    "dusk.os.linux.os_release.version_id",
    "dusk.os.android.release",
    "dusk.os.android.sdk",
    "dusk.os.android.security_patch",
    "dusk.os.android.incremental",
    "dusk.os.android.model",
    "dusk.os.android.manufacturer",
    "dusk.os.android.fingerprint",
    "dusk.os.android.brand",
    "dusk.os.android.build_type",
    "dusk.os.android.abi_list",
    "dusk.os.macos.product_version",
    "dusk.os.macos.build_version",
    "dusk.os.macos.translated",
    "dusk.os.ios.product_version",
    "dusk.os.ios.build_version",
    "dusk.os.windows.major_version",
    "dusk.os.windows.minor_version",
    "dusk.os.windows.build_number",
    "dusk.os.windows.revision",
    "dusk.os.windows.edition",
    "dusk.os.windows.display_version",
    "dusk.os.windows.native_arch",
    "dusk.os.windows.emulated",
    "dusk.os.windows.computer_name",
    "dusk.os.windows.session_id",
    "dusk.os.windows.elevated",
    "dusk.device.cores",
    "dusk.device.memory_bytes",
    "dusk.device.swap_bytes",
    "dusk.device.boot_time_ms",
    "dusk.device.vendor",
    "dusk.device.model",
    "dusk.device.cpu",
    "dusk.device.id",
];

static DUSK_KEYS: [u64; 71] = dusk_program_kvs_internal::key_ids(DUSK_KEY_NAMES);

#[cfg(feature = "client")]
dusk_program_kvs_internal::known_keys!(DUSK_KEY_LIST, &DUSK_KEY_NAMES);

pub type InitArgsDataMessage = ImbuedMessageBuilder<HeapAllocator>;

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: InitArgsDataMessage,
}

impl Args {
    pub fn new(init_script: &[u8]) -> capnp::Result<Self> {
        let message =
            capnp::serialize::read_message(init_script, capnp::message::ReaderOptions::new())?;
        let mut data = InitArgsDataMessage::new(HeapAllocator::new());
        data.get_root::<init_capnp::init_args::data::Builder>()?
            .set_init_script(message.get_root::<bytecode::Reader>()?)?;
        Ok(Args { data })
    }
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher {
    tid: u64,
}

impl Default for Launcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Launcher {
    pub fn new() -> Self {
        let tid = dusk_core::driver::tid();
        dusk_program_kvs_internal::own_keys(tid, &DUSK_KEYS);
        Self { tid }
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        dusk_program_kvs_internal::disown_keys(self.tid, &DUSK_KEYS);
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
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    pub async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self> {
        Ok(Process { ctx })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> dusk_capnp::dusk_capnp::portal::Client {
        let client: init_capnp::init_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<dusk_capnp::dusk_capnp::portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        let namespace_id = self.ctx.namespace.id;
        let kvs = dusk_program_kvs_internal::get_kvs(namespace_id);
        let sticky = dusk_program_kvs_internal::FLAG_STICKY;
        kvs.set(
            VERSION_KEY,
            Value::String(String::from(dusk_capnp::VERSION)),
            sticky,
        )
        .await;
        kvs.set(
            GIT_REV_KEY,
            Value::String(String::from(dusk_capnp::GIT_REV)),
            sticky,
        )
        .await;
        kvs.set(NAMESPACE_ID_KEY, Value::Uint(namespace_id), sticky)
            .await;
        kvs.set(TID_KEY, Value::Uint(dusk_core::driver::tid()), sticky)
            .await;
        match dusk_core::driver::hostname() {
            Ok(hostname) => kvs.set(HOSTNAME_KEY, Value::String(hostname), sticky).await,
            Err(error) => tracing::warn!("couldn't read the hostname for dusk.hostname: {error:#}"),
        }

        let arch = env!("DUSK_TARGET_ARCH");
        let os = env!("DUSK_TARGET_OS");
        let bits = u64::from(usize::BITS);
        kvs.set(ARCH_KEY, Value::String(String::from(arch)), sticky)
            .await;
        kvs.set(OS_KEY, Value::String(String::from(os)), sticky)
            .await;
        kvs.set(BITS_KEY, Value::Uint(bits), sticky).await;
        tracing::info!(arch, os, bits, "dusk target");
        match kvs.get(IMPL_KEY).await {
            Some(Value::String(name)) => tracing::info!(name = name.as_str(), "dusk impl"),
            Some(value) => tracing::warn!(value = ?value, "dusk.impl is not a string"),
            None => tracing::warn!("dusk.impl is not set"),
        }

        ready.sender().send(true);

        let dusk_client = dusk_core::local_client(self.namespace().clone()).await;
        let programs_response = dusk_client.programs_request().send().promise.await?;
        let program_entries = programs_response.get()?.get_program_entries()?;
        for entry in program_entries.iter() {
            tracing::info!(
                program_id = entry.get_program_id(),
                version = entry.get_version()?.to_str()?,
                git_rev = entry.get_git_revision()?.to_str()?,
                "available program",
            );
        }

        let init_script = self
            .ctx
            .program_args
            .with_data::<init_capnp::init_args::data::Owned, _, _>(|data| {
                let mut init_script = BytecodeMessage::new(HeapAllocator::new());
                init_script.set_root::<bytecode::Owned>(data.get_init_script()?)?;
                Ok(init_script)
            })?;
        let sh_args =
            dusk_program_sh::ShArgs::new(dusk_program_sh::ShMode::DetachedScript(init_script))?
                .as_program_args()?;

        let mut process_request = dusk_client.process_request();
        sh_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
        let process = process_request.send().promise.await?.get()?.get_result()?;
        let mut run_request = dusk_client.run_request();
        run_request.get().set_process(process.clone());
        run_request.send().promise.await?;
        let pid = process
            .pid_request()
            .send()
            .promise
            .await?
            .get()?
            .get_result();

        let mut waitpid_request = dusk_client.waitpid_request();
        waitpid_request.get().set_pid(pid);
        let reap = async {
            if let Err(error) = waitpid_request.send().promise.await {
                tracing::warn!(pid, error = %error, "waitpid on the sh running the init script failed");
            }
            core::future::pending::<()>().await
        };
        let terminate = async {
            loop {
                if let Signal::Terminate = signal_receiver.receive().await {
                    return;
                }
            }
        };
        embassy_futures::select::select(reap, terminate).await;
        Ok(())
    }
}

#[derive(dusk_program_proc::Portal)]
pub struct Portal {
    pub process: Process,
}

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {}
