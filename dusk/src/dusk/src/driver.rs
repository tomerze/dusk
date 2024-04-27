use alloc::string::String;
use anyhow::Result;
use dusk_capnp::dusk_capnp::program_args;
use dusk_program::Process;

/// Dusk driver
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    fn hostname(&self, namespace: u64) -> Result<String>;

    fn exec(&self, namespace: u64, program_args: program_args::Client) -> Result<Box<dyn Process>>;
}

/// Set the dusk Driver implementation.
#[macro_export]
macro_rules! dusk_driver_impl {
    (static ref $name:ident: $t: ty = $val:expr) => {
        lazy_static::lazy_static! {
            static ref $name: $t = $val;
        }

        #[no_mangle]
        fn _dusk_hostname(namespace: u64) -> Result<String> {
            <$t as $crate::driver::Driver>::hostname(&$name, namespace)
        }

        #[no_mangle]
        fn _dusk_exec<'a>(
            namespace: u64,
            program_args: dusk::dusk_capnp::dusk_capnp::program_args::Client,
        ) -> Result<Box<dyn Process>> {
            <$t as $crate::driver::Driver>::exec(&$name, namespace, program_args)
        }
    };
}

extern "Rust" {
    fn _dusk_hostname(namespace: u64) -> Result<String>;

    fn _dusk_exec<'a>(
        namespace: u64,
        program_args: program_args::Client,
    ) -> Result<Box<dyn Process>>;
}

pub fn hostname(namespace: u64) -> Result<String> {
    unsafe { _dusk_hostname(namespace) }
}

pub fn exec<'a>(namespace: u64, program_args: program_args::Client) -> Result<Box<dyn Process>> {
    unsafe { _dusk_exec(namespace, program_args) }
}
