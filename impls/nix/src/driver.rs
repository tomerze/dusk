use crate::fs::NixFsDriver;
use dusk_core::driver::{Driver, FsDriver};
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

    fn fs_driver(&self) -> Result<Box<dyn FsDriver>> {
        Ok(Box::new(NixFsDriver))
    }

    fn exit(&self, exit_code: i32) {
        std::panic::panic_any(crate::ExitCode(exit_code));
    }
}
