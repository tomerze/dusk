use crate::{portal::ShPortal, sh_capnp};
use alloc::rc::Rc;
use anyhow::Result;
use dusk_capnp::dusk_capnp::portal;
use dusk_program::{namespace::Namespace, process::Process};
use embassy_time::Timer;

pub struct ShProcess {
    pub pid: u64,
    pub namespace: Rc<Namespace>,
    pub program_args: sh_capnp::sh_args::Client,
}

#[async_trait::async_trait(?Send)]
impl Process for ShProcess {
    fn pid(&self) -> u64 {
        self.pid
    }
    fn program_id(&self) -> u64 {
        sh_capnp::PROGRAM_ID
    }
    fn namespace(&self) -> Rc<Namespace> {
        self.namespace.clone()
    }
    fn portal(&self) -> portal::Client {
        capnp_rpc::new_client(ShPortal {})
    }
    fn clone_box(&self) -> Box<dyn Process> {
        Box::new(ShProcess {
            pid: self.pid,
            namespace: self.namespace.clone(),
            program_args: self.program_args.clone(),
        })
    }

    async fn main(&self) -> Result<()> {
        Timer::after_secs(5).await;
        Ok(())
    }
}
