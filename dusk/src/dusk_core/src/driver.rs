use alloc::string::String;
use dusk_program::anyhow::Result;

/// Dusk driver.
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    fn hostname(&self) -> Result<String>;

    fn exit(&self, exit_code: i32);
}

/// Set the dusk Driver implementation.
#[macro_export]
macro_rules! dusk_driver_impl {
    (static ref $name:ident: $t: ty = $val:expr) => {
        lazy_static::lazy_static! {
            static ref $name: $t = $val;
        }

        #[unsafe(no_mangle)]
        fn _dusk_hostname() -> Result<String> {
            <$t as $crate::driver::Driver>::hostname(&$name)
        }

        #[unsafe(no_mangle)]
        fn _dusk_exit(exit_code: i32) {
            <$t as $crate::driver::Driver>::exit(&$name, exit_code)
        }
    };
}

unsafe extern "Rust" {
    fn _dusk_hostname() -> Result<String>;

    fn _dusk_exit(exit_code: i32);
}

pub fn hostname() -> Result<String> {
    unsafe { _dusk_hostname() }
}

pub fn exit(exit_code: i32) {
    unsafe { _dusk_exit(exit_code) }
}
