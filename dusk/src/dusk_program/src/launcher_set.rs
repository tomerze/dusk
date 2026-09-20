use crate::launcher::{Launcher, LauncherMixin};
use crate::prelude::ProcessContext;
use crate::process::Process;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use anyhow::{Result, anyhow};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

type LauncherVec = Vec<Box<dyn Launcher + Send>>;

#[derive(Clone)]
pub struct LauncherSet {
    pub launchers: Arc<Mutex<CriticalSectionRawMutex, LauncherVec>>,
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

    pub fn from_launchers(launchers: Vec<Box<dyn Launcher + Send>>) -> Self {
        let launchers = Arc::new(Mutex::<CriticalSectionRawMutex, LauncherVec>::new(
            launchers,
        ));
        LauncherSet { launchers }
    }

    pub fn add(&self, launcher: Box<dyn Launcher + Send>) {
        embassy_futures::block_on(async {
            let mut launchers = self.launchers.lock().await;
            launchers.push(launcher);
        });
    }

    pub async fn launch(&self, process_context: ProcessContext) -> Result<Box<dyn Process>> {
        let mut launchers = self.launchers.lock().await;
        let program_id = process_context.program_args.program_id()?;
        for launcher in launchers.iter_mut() {
            if launcher.program_id() == program_id {
                let pid = process_context.pid;
                let result = launcher.launch(process_context).await;
                tracing::info!(
                    pid,
                    program_id,
                    error = result.as_ref().err().map(tracing::field::debug),
                    "process launch",
                );
                return result;
            }
        }
        Err(anyhow!("no launcher found for program id {}", program_id))
    }
}

#[async_trait::async_trait(?Send)]
impl LauncherMixin for LauncherSet {
    async fn launch(&mut self, process_context: ProcessContext) -> Result<Box<dyn Process>> {
        LauncherSet::launch(self, process_context).await
    }
}
