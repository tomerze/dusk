use dusk_core::driver::Driver;
use dusk_program::anyhow::{Ok, Result, anyhow};

pub(crate) struct StdDriver;

dusk_core::dusk_driver_impl!(static ref DRIVER: StdDriver = StdDriver);

impl Driver for StdDriver {
    fn hostname(&self) -> Result<String> {
        Ok(std::env::consts::OS.to_string())
    }

    fn random(&self, buffer: &mut [u8]) -> Result<()> {
        getrandom::getrandom(buffer)
            .map_err(|error| anyhow!("the operating system's random source failed: {error}"))
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

    #[test]
    fn test_random_fills_the_buffer() {
        let mut direct = [0u8; 32];
        let mut through_the_shim = [0u8; 32];
        StdDriver.random(&mut direct).unwrap();
        dusk_core::driver::random(&mut through_the_shim).unwrap();
        assert_ne!(direct, [0u8; 32]);
        assert_ne!(through_the_shim, [0u8; 32]);
        assert_ne!(direct, through_the_shim);
    }
}
