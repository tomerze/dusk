use core::{future::Future, pin::Pin};

use alloc::{rc::Rc, string::String};
use dusk_program::anyhow::Result;
use dusk_program::program_args::ProgramArgs;
use dusk_program::{namespace::Namespace, process::Process};

pub type FutureProcessResult = Pin<Box<dyn Future<Output = Result<Box<dyn Process>>>>>;

/// Dusk driver.
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    fn hostname(&self) -> Result<String>;

    fn process(
        &self,
        namespace: Rc<Namespace>,
        program_args: Rc<ProgramArgs>,
    ) -> FutureProcessResult;
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
        fn _dusk_process<'a>(
            namespace: Rc<Namespace>,
            program_args: Rc<dusk_program::program_args::ProgramArgs>,
        ) -> FutureProcessResult {
            <$t as $crate::driver::Driver>::process(&$name, namespace, program_args)
        }
    };
}

unsafe extern "Rust" {
    fn _dusk_hostname() -> Result<String>;

    fn _dusk_process<'a>(
        namespace: Rc<Namespace>,
        program_args: Rc<ProgramArgs>,
    ) -> FutureProcessResult;
}

pub fn hostname() -> Result<String> {
    unsafe { _dusk_hostname() }
}

pub fn process(namespace: Rc<Namespace>, program_args: Rc<ProgramArgs>) -> FutureProcessResult {
    unsafe { _dusk_process(namespace, program_args) }
}
