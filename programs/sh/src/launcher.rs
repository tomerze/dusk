use crate::process::ShProcess;
use alloc::rc::Rc;
use anyhow::Result;
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::program_args;
use dusk_program::{launcher::Launcher, namespace::Namespace, process::Process};

pub struct ShLauncher {}

impl Launcher for ShLauncher {
    fn program_id(&self) -> u64 {
        crate::sh_capnp::PROGRAM_ID
    }
    fn launch(
        &mut self,
        pid: u64,
        namespace: Rc<Namespace>,
        program_args: program_args::Client,
    ) -> Result<Box<dyn Process>> {
        let cast_program_args = program_args.cast_to::<crate::sh_capnp::sh_args::Client>();
        Ok(Box::new(ShProcess {
            pid,
            namespace,
            program_args: cast_program_args,
        }))
    }
}
