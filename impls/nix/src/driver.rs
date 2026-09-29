use dusk_core::driver::Driver;
use dusk_program::anyhow::{Ok, Result, anyhow};
use nix::unistd::gethostname;

pub(crate) struct NixDriver;

dusk_core::dusk_driver_impl!(static ref DRIVER: NixDriver = NixDriver);

impl Driver for NixDriver {
    fn hostname(&self) -> Result<String> {
        Ok(gethostname()?
            .into_string()
            .map_err(|os_str| anyhow!("failed to parse hostname `{os_str:#?}` to UTF-8"))?)
    }

    fn exit(&self, exit_code: i32) {
        crate::exit(exit_code);
    }
}
