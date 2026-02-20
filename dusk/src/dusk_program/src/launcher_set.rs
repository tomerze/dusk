use crate::launcher::Launcher;
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
        let program_id = process_context
            .program_args
            .program_id_request()
            .send()
            .promise
            .await?
            .get()?
            .get_program_id();
        for launcher in launchers.iter_mut() {
            if launcher.program_id() == program_id {
                return launcher.launch(process_context);
            }
        }
        Err(anyhow!("no launcher found for program id {}", program_id))
    }
}

pub trait LauncherSetBuilder: Send + Sync {
    fn build(&self) -> Result<LauncherSet>;
}

pub struct StatelessLauncherSetBuilder {
    launcher_set: LauncherSet,
}

impl StatelessLauncherSetBuilder {
    pub fn new(launcher_set: LauncherSet) -> Self {
        StatelessLauncherSetBuilder { launcher_set }
    }
}
impl LauncherSetBuilder for StatelessLauncherSetBuilder {
    fn build(&self) -> Result<LauncherSet> {
        Ok(self.launcher_set.clone())
    }
}
