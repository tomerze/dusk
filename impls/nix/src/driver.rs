use alloc::rc::Rc;

use anyhow::{anyhow, Ok, Result};
use core::future::Future;
use core::pin::Pin;
use dusk::driver::{Driver, FutureProcessResult};
use dusk_program::launcher_set::LauncherSet;
use dusk_program::{namespace::Namespace, process::Process};
use dusk_program_sh::launcher::ShLauncher;
use nix::{sys::time::TimeValLike, unistd::gethostname};
use rand::Rng;

struct NixDriver {
    launchers: LauncherSet,
}

impl NixDriver {
    fn new() -> Self {
        let launchers = LauncherSet::new();
        launchers.add(Box::new(ShLauncher {}));
        NixDriver { launchers }
    }
}

dusk::dusk_driver_impl!(static ref DRIVER: NixDriver = NixDriver::new());

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
        let launchers = self.launchers.clone();
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
