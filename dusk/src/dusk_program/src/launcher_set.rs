use crate::launcher::Launcher;
use crate::namespace::Namespace;
use crate::process::Process;
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::{rc::Rc, sync::Arc};
use anyhow::{anyhow, Result};
use dusk_capnp::dusk_capnp::program_args;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

type LauncherVec = Vec<Box<dyn Launcher + Send>>;

#[derive(Clone)]
pub struct LauncherSet {
    launchers: Arc<Mutex<CriticalSectionRawMutex, LauncherVec>>,
}

impl Default for LauncherSet {
    fn default() -> Self {
        Self::new()
    }
}

impl LauncherSet {
    pub fn new() -> Self {
        let launchers = Arc::new(Mutex::<CriticalSectionRawMutex, LauncherVec>::new(
            Vec::new(),
        ));
        LauncherSet { launchers }
    }

    pub fn add(&self, launcher: Box<dyn Launcher + Send>) {
        embassy_futures::block_on(async {
            let mut launchers = self.launchers.lock().await;
            launchers.push(launcher);
        });
    }

    pub async fn launch(
        &self,
        pid: u64,
        namespace: Rc<Namespace>,
        program_args: program_args::Client,
    ) -> Result<Box<dyn Process>> {
        let mut launchers = self.launchers.lock().await;
        let program_id = program_args
            .program_id_request()
            .send()
            .promise
            .await?
            .get()?
            .get_program_id();
        for launcher in launchers.iter_mut() {
            if launcher.program_id() == program_id {
                return launcher.launch(pid, namespace, program_args);
            }
        }
        Err(anyhow!("no launcher found for program id {}", program_id))
    }
}
