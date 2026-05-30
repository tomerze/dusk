use alloc::rc::Rc;
use dusk_core::driver::Driver;
use dusk_program::anyhow::{Ok, Result, anyhow};
use dusk_program::launcher_set::{LauncherSet, LauncherSetBuilder};
use dusk_program::namespace::Namespace;
use nix::unistd::gethostname;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) struct NixDriver {
    launcher_set_builders: Mutex<HashMap<u64, Box<dyn LauncherSetBuilder>>>,
}

impl NixDriver {
    fn new() -> Self {
        NixDriver {
            launcher_set_builders: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_launcher_set_builder(
        &self,
        namespace_id: u64,
        builder: impl LauncherSetBuilder + 'static,
    ) {
        self.launcher_set_builders
            .lock()
            .unwrap()
            .insert(namespace_id, Box::new(builder));
    }
}

dusk_core::dusk_driver_impl!(static ref DRIVER: NixDriver = NixDriver::new());

pub(crate) fn driver() -> &'static NixDriver {
    &DRIVER
}

impl Driver for NixDriver {
    fn hostname(&self) -> Result<String> {
        Ok(gethostname()?
            .into_string()
            .map_err(|os_str| anyhow!("failed to parse hostname `{os_str:#?}` to UTF-8"))?)
    }

    fn exit(&self, exit_code: i32) {
        let mut launcher_set_builders = self.launcher_set_builders.lock().unwrap();
        launcher_set_builders.clear();
        std::panic::panic_any(crate::ExitCode(exit_code));
    }

    fn launchers(&self, namespace: Rc<Namespace>) -> Result<LauncherSet> {
        let launcher_set_builders = self.launcher_set_builders.lock().unwrap();
        let launcher_set_builder = launcher_set_builders.get(&namespace.id).ok_or_else(|| {
            anyhow!(
                "launcher set builder not found for namespace {}",
                namespace.id
            )
        })?;

        launcher_set_builder.build()
    }
}
