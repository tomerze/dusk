use crate::{namespace::Namespace, process::Process};
use alloc::{boxed::Box, rc::Rc};
use anyhow::Result;
use dusk_capnp::dusk_capnp::program_args;

pub trait Launcher {
    fn program_id(&self) -> u64;
    fn launch(
        &mut self,
        pid: u64,
        namespace: Rc<Namespace>,
        program_args: program_args::Client,
    ) -> Result<Box<dyn Process>>;
}

#[macro_export]
macro_rules! basic_launcher {
    ($launcher_name:ident, $program_id:expr, $process_type:ty, $args_type:path) => {
        pub struct $launcher_name {}

        impl dusk_program::launcher::Launcher for $launcher_name {
            fn program_id(&self) -> u64 {
                $program_id
            }
            fn launch(
                &mut self,
                pid: u64,
                namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
                program_args: dusk_capnp::dusk_capnp::program_args::Client,
            ) -> anyhow::Result<Box<dyn dusk_program::process::Process>> {
                let cast_program_args =
                    capnp::capability::FromClientHook::cast_to::<$args_type>(program_args);
                Ok(Box::new(<$process_type>::new(
                    pid,
                    namespace,
                    cast_program_args,
                )))
            }
        }
    };
}
