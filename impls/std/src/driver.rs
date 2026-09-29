use crate::fs::StdFsDriver;
use dusk_core::driver::{Driver, FsDriver};
use dusk_program::anyhow::{Ok, Result};

pub(crate) struct StdDriver;

dusk_core::dusk_driver_impl!(static ref DRIVER: StdDriver = StdDriver);

impl Driver for StdDriver {
    fn hostname(&self) -> Result<String> {
        Ok(std::env::consts::OS.to_string())
    }

    fn fs_driver(&self) -> Result<Box<dyn FsDriver>> {
        Ok(Box::new(StdFsDriver))
    }

    fn exit(&self, exit_code: i32) {
        tracing::info!(exit_code, "node exiting");
        std::panic::panic_any(crate::ExitCode(exit_code));
    }
}
