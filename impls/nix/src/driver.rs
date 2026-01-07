use alloc::rc::Rc;

use anyhow::{anyhow, Ok, Result};
use core::future::Future;
use core::pin::Pin;
use dusk::driver::{Driver, FutureProcessResult};
use dusk_program::launcher_set::{LauncherSet, LauncherSetBuilder};
use dusk_program::{namespace::Namespace, process::Process};
use nix::{sys::time::TimeValLike, unistd::gethostname};
use rand::Rng;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) struct NixDriver {
    launcher_set_builders: Mutex<HashMap<u64, Box<dyn LauncherSetBuilder>>>,
}

impl NixDriver {
    fn new() -> Self {
        NixDriver {
            launcher_set_builders: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_launcher_set_builder(
        &self,
        namespace_id: u64,
        builder: impl LauncherSetBuilder + 'static,
    ) {
        self.launcher_set_builders
            .lock()
            .unwrap()
            .insert(namespace_id, Box::new(builder));
    }

    fn launchers(&self, namespace_id: u64) -> Result<LauncherSet> {
        let launcher_set_builders = self.launcher_set_builders.lock().unwrap();
        let launcher_set_builder = launcher_set_builders.get(&namespace_id).ok_or_else(|| {
            anyhow!(
                "launcher set builder not found for namespace {}",
                namespace_id
            )
        })?;

        launcher_set_builder.build()
    }
}

dusk::dusk_driver_impl!(static ref DRIVER: NixDriver = NixDriver::new());

pub(crate) fn driver() -> &'static NixDriver {
    &DRIVER
}

impl Driver for NixDriver {
    fn hostname(&self) -> Result<String> {
        Ok(gethostname()?
            .into_string()
            .map_err(|os_str| anyhow!("failed to parse hostname `{os_str:#?}` to UTF-8"))?)
    }

    fn process(
        &self,
        namespace: Rc<Namespace>,
        program_args: dusk::dusk_capnp::dusk_capnp::program_args::Client,
    ) -> FutureProcessResult {
        let namespace_id = namespace.id;
        let launchers = match self.launchers(namespace_id) {
            core::result::Result::Ok(l) => l,
            Err(e) => return Box::pin(async move { Err(e) }),
        };
        let fut = async move {
            let mut rng = rand::thread_rng();
            let pid: u64 = rng.gen();
            launchers.launch(pid, namespace, program_args).await
        };
        Box::pin(fut)
            as Pin<Box<dyn Future<Output = Result<Box<dyn Process>, anyhow::Error>> + 'static>>
    }

    fn now(&self) -> Result<embassy_time::Instant> {
        Ok(embassy_time::Instant::from_micros(
            nix::time::clock_gettime(nix::time::ClockId::CLOCK_REALTIME)?
                .num_microseconds()
                .try_into()
                .map_err(|err| anyhow!("system time is set before unix epoch: `{err}`"))?,
        ))
    }
}
