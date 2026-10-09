use crate::fs::WindowsFsDriver;
use dusk_core::driver::{Driver, FsDriver};
use dusk_program::anyhow::{Ok, Result, anyhow};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::System::WindowsProgramming::GetComputerNameW;

pub(crate) struct WindowsDriver;

dusk_core::dusk_driver_impl!(static ref DRIVER: WindowsDriver = WindowsDriver);

impl Driver for WindowsDriver {
    fn hostname(&self) -> Result<String> {
        let mut name = [0u16; 16];
        let mut length = name.len() as u32;
        if unsafe { GetComputerNameW(name.as_mut_ptr(), &mut length) } == 0 {
            return Err(anyhow!(
                "GetComputerNameW failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(String::from_utf16(&name[..length as usize])?)
    }

    fn tid(&self) -> u64 {
        u64::from(unsafe { GetCurrentThreadId() })
    }

    fn fs_driver(&self) -> Result<Box<dyn FsDriver>> {
        Ok(Box::new(WindowsFsDriver))
    }

    fn exit(&self, exit_code: i32) {
        std::panic::panic_any(crate::ExitCode(exit_code));
    }
}
