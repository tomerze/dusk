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

#[cfg(feature = "client")]
mod known_keys {
    dusk_program_kvs_internal::known_key!(VERSION, "dusk.version");
    dusk_program_kvs_internal::known_key!(GIT_REV, "dusk.git_rev");
    dusk_program_kvs_internal::known_key!(NAMESPACE_ID, "dusk.namespace_id");
    dusk_program_kvs_internal::known_key!(TID, "dusk.tid");
    dusk_program_kvs_internal::known_key!(HOSTNAME, "dusk.hostname");
    dusk_program_kvs_internal::known_key!(ARCH, "dusk.target.arch");
    dusk_program_kvs_internal::known_key!(OS, "dusk.target.os");
    dusk_program_kvs_internal::known_key!(BITS, "dusk.target.bits");
    dusk_program_kvs_internal::known_key!(IMPL, "dusk.impl");
    dusk_program_kvs_internal::known_key!(OS_PROCESS_PID, "dusk.os.process.pid");
    dusk_program_kvs_internal::known_key!(OS_PROCESS_EXECUTABLE, "dusk.os.process.executable");
    dusk_program_kvs_internal::known_key!(
        OS_PROCESS_WORKING_DIRECTORY,
        "dusk.os.process.working_directory"
    );
    dusk_program_kvs_internal::known_key!(OS_PROCESS_PARENT_PID, "dusk.os.process.parent_pid");
    dusk_program_kvs_internal::known_key!(OS_TIME_ZONE, "dusk.os.time_zone");
    dusk_program_kvs_internal::known_key!(OS_NIX_UNAME_SYSNAME, "dusk.os.nix.uname.sysname");
    dusk_program_kvs_internal::known_key!(OS_NIX_UNAME_NODENAME, "dusk.os.nix.uname.nodename");
    dusk_program_kvs_internal::known_key!(OS_NIX_UNAME_RELEASE, "dusk.os.nix.uname.release");
    dusk_program_kvs_internal::known_key!(OS_NIX_UNAME_VERSION, "dusk.os.nix.uname.version");
    dusk_program_kvs_internal::known_key!(OS_NIX_UNAME_MACHINE, "dusk.os.nix.uname.machine");
    dusk_program_kvs_internal::known_key!(OS_NIX_UNAME_DOMAINNAME, "dusk.os.nix.uname.domainname");
    dusk_program_kvs_internal::known_key!(OS_NIX_UID, "dusk.os.nix.uid");
    dusk_program_kvs_internal::known_key!(OS_NIX_EUID, "dusk.os.nix.euid");
    dusk_program_kvs_internal::known_key!(
        OS_NIX_LIMITS_OPEN_FILES_SOFT,
        "dusk.os.nix.limits.open_files.soft"
    );
    dusk_program_kvs_internal::known_key!(
        OS_NIX_LIMITS_OPEN_FILES_HARD,
        "dusk.os.nix.limits.open_files.hard"
    );
    dusk_program_kvs_internal::known_key!(
        OS_NIX_LIMITS_CORE_FILE_SIZE_SOFT,
        "dusk.os.nix.limits.core_file_size.soft"
    );
    dusk_program_kvs_internal::known_key!(
        OS_NIX_LIMITS_CORE_FILE_SIZE_HARD,
        "dusk.os.nix.limits.core_file_size.hard"
    );
    dusk_program_kvs_internal::known_key!(OS_LINUX_BOOT_ID, "dusk.os.linux.boot_id");
    dusk_program_kvs_internal::known_key!(OS_LINUX_PID1, "dusk.os.linux.pid1");
    dusk_program_kvs_internal::known_key!(OS_LINUX_GLIBC_VERSION, "dusk.os.linux.glibc_version");
    dusk_program_kvs_internal::known_key!(OS_LINUX_OS_RELEASE_ID, "dusk.os.linux.os_release.id");
    dusk_program_kvs_internal::known_key!(
        OS_LINUX_OS_RELEASE_ID_LIKE,
        "dusk.os.linux.os_release.id_like"
    );
    dusk_program_kvs_internal::known_key!(
        OS_LINUX_OS_RELEASE_NAME,
        "dusk.os.linux.os_release.name"
    );
    dusk_program_kvs_internal::known_key!(
        OS_LINUX_OS_RELEASE_PRETTY_NAME,
        "dusk.os.linux.os_release.pretty_name"
    );
    dusk_program_kvs_internal::known_key!(
        OS_LINUX_OS_RELEASE_VERSION,
        "dusk.os.linux.os_release.version"
    );
    dusk_program_kvs_internal::known_key!(
        OS_LINUX_OS_RELEASE_VERSION_CODENAME,
        "dusk.os.linux.os_release.version_codename"
    );
    dusk_program_kvs_internal::known_key!(
        OS_LINUX_OS_RELEASE_VERSION_ID,
        "dusk.os.linux.os_release.version_id"
    );
    dusk_program_kvs_internal::known_key!(OS_ANDROID_RELEASE, "dusk.os.android.release");
    dusk_program_kvs_internal::known_key!(OS_ANDROID_SDK, "dusk.os.android.sdk");
    dusk_program_kvs_internal::known_key!(
        OS_ANDROID_SECURITY_PATCH,
        "dusk.os.android.security_patch"
    );
    dusk_program_kvs_internal::known_key!(OS_ANDROID_INCREMENTAL, "dusk.os.android.incremental");
    dusk_program_kvs_internal::known_key!(OS_ANDROID_MODEL, "dusk.os.android.model");
    dusk_program_kvs_internal::known_key!(OS_ANDROID_MANUFACTURER, "dusk.os.android.manufacturer");
    dusk_program_kvs_internal::known_key!(OS_ANDROID_FINGERPRINT, "dusk.os.android.fingerprint");
    dusk_program_kvs_internal::known_key!(OS_ANDROID_BRAND, "dusk.os.android.brand");
    dusk_program_kvs_internal::known_key!(OS_ANDROID_BUILD_TYPE, "dusk.os.android.build_type");
    dusk_program_kvs_internal::known_key!(OS_ANDROID_ABI_LIST, "dusk.os.android.abi_list");
    dusk_program_kvs_internal::known_key!(
        OS_MACOS_PRODUCT_VERSION,
        "dusk.os.macos.product_version"
    );
    dusk_program_kvs_internal::known_key!(OS_MACOS_BUILD_VERSION, "dusk.os.macos.build_version");
    dusk_program_kvs_internal::known_key!(OS_MACOS_TRANSLATED, "dusk.os.macos.translated");
    dusk_program_kvs_internal::known_key!(OS_IOS_PRODUCT_VERSION, "dusk.os.ios.product_version");
    dusk_program_kvs_internal::known_key!(OS_IOS_BUILD_VERSION, "dusk.os.ios.build_version");
    dusk_program_kvs_internal::known_key!(
        OS_WINDOWS_MAJOR_VERSION,
        "dusk.os.windows.major_version"
    );
    dusk_program_kvs_internal::known_key!(
        OS_WINDOWS_MINOR_VERSION,
        "dusk.os.windows.minor_version"
    );
    dusk_program_kvs_internal::known_key!(OS_WINDOWS_BUILD_NUMBER, "dusk.os.windows.build_number");
    dusk_program_kvs_internal::known_key!(OS_WINDOWS_REVISION, "dusk.os.windows.revision");
    dusk_program_kvs_internal::known_key!(OS_WINDOWS_EDITION, "dusk.os.windows.edition");
    dusk_program_kvs_internal::known_key!(
        OS_WINDOWS_DISPLAY_VERSION,
        "dusk.os.windows.display_version"
    );
    dusk_program_kvs_internal::known_key!(OS_WINDOWS_NATIVE_ARCH, "dusk.os.windows.native_arch");
    dusk_program_kvs_internal::known_key!(OS_WINDOWS_EMULATED, "dusk.os.windows.emulated");
    dusk_program_kvs_internal::known_key!(
        OS_WINDOWS_COMPUTER_NAME,
        "dusk.os.windows.computer_name"
    );
    dusk_program_kvs_internal::known_key!(OS_WINDOWS_SESSION_ID, "dusk.os.windows.session_id");
    dusk_program_kvs_internal::known_key!(OS_WINDOWS_ELEVATED, "dusk.os.windows.elevated");
    dusk_program_kvs_internal::known_key!(DEVICE_CORES, "dusk.device.cores");
    dusk_program_kvs_internal::known_key!(DEVICE_MEMORY_BYTES, "dusk.device.memory_bytes");
    dusk_program_kvs_internal::known_key!(DEVICE_SWAP_BYTES, "dusk.device.swap_bytes");
    dusk_program_kvs_internal::known_key!(DEVICE_BOOT_TIME_MS, "dusk.device.boot_time_ms");
    dusk_program_kvs_internal::known_key!(DEVICE_VENDOR, "dusk.device.vendor");
    dusk_program_kvs_internal::known_key!(DEVICE_MODEL, "dusk.device.model");
    dusk_program_kvs_internal::known_key!(DEVICE_CPU, "dusk.device.cpu");
    dusk_program_kvs_internal::known_key!(DEVICE_ID, "dusk.device.id");
}

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
        kvs.set(
            VERSION_KEY,
            Value::String(String::from(dusk_capnp::VERSION)),
        )
        .await;
        kvs.set(
            GIT_REV_KEY,
            Value::String(String::from(dusk_capnp::GIT_REV)),
        )
        .await;
        kvs.set(NAMESPACE_ID_KEY, Value::Uint(namespace_id)).await;
        kvs.set(TID_KEY, Value::Uint(dusk_core::driver::tid()))
            .await;
        match dusk_core::driver::hostname() {
            Ok(hostname) => kvs.set(HOSTNAME_KEY, Value::String(hostname)).await,
            Err(error) => tracing::warn!("couldn't read the hostname for dusk.hostname: {error:#}"),
        }

        let arch = env!("DUSK_TARGET_ARCH");
        let os = env!("DUSK_TARGET_OS");
        let bits = u64::from(usize::BITS);
        kvs.set(ARCH_KEY, Value::String(String::from(arch))).await;
        kvs.set(OS_KEY, Value::String(String::from(os))).await;
        kvs.set(BITS_KEY, Value::Uint(bits)).await;
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
