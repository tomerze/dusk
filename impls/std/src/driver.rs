use dusk_core::driver::Driver;
use dusk_program::anyhow::{Ok, Result};

pub(crate) struct StdDriver;

dusk_core::dusk_driver_impl!(static ref DRIVER: StdDriver = StdDriver);

impl Driver for StdDriver {
    fn hostname(&self) -> Result<String> {
        Ok(std::env::consts::OS.to_string())
    }

    fn tid(&self) -> u64 {
        std::thread::current().id().as_u64().get()
    }

    fn exit(&self, exit_code: i32) {
        tracing::info!(exit_code, "node exiting");
        std::panic::panic_any(crate::ExitCode(exit_code));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tid_names_the_calling_thread() {
        let here = StdDriver.tid();
        assert_ne!(here, 0);
        assert_eq!(StdDriver.tid(), here);
        let there = std::thread::spawn(|| StdDriver.tid()).join().unwrap();
        assert_ne!(there, 0);
        assert_ne!(there, here);
    }
}
