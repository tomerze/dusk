use anyhow::{anyhow, Ok, Result};
use dusk::driver::Driver;
use dusk_program::{Launcher, Process};
use dusk_program_sh::launcher::ShLauncher;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use nix::{sys::time::TimeValLike, unistd::gethostname};

struct NixDriver {
    sh: Mutex<CriticalSectionRawMutex, ShLauncher>,
}

impl NixDriver {
    fn new() -> Self {
        let sh = Mutex::<CriticalSectionRawMutex, ShLauncher>::new(ShLauncher::new());

        NixDriver { sh }
    }
}

dusk::dusk_driver_impl!(static ref DRIVER: NixDriver = NixDriver::new());

impl Driver for NixDriver {
    fn hostname(&self, _namespace: u64) -> Result<String> {
        Ok(String::from(gethostname()?.into_string().map_err(
            |os_str| anyhow!("failed to parse hostname `{os_str:#?}` to UTF-8"),
        )?))
    }

    fn create_process(
        &mut self,
        namespace: u64,
        program_args: dusk::dusk_capnp::dusk_capnp::program_args::Client,
    ) -> Result<Box<dyn Process>> {
        let sh = self.sh.lock();

        Ok(sh.launch())
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
