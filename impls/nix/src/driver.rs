use anyhow::{anyhow, Result};
use dusk::driver::Driver;
use dusk_program::Process;
use dusk_program_sh::launcher::ShLauncher;
use nix::unistd::gethostname;

struct NixDriver {
    sh: ShLauncher,
}

impl NixDriver {
    fn new() -> Self {
        let sh = ShLauncher::new();

        NixDriver { sh }
    }
}

dusk::dusk_driver_impl!(static DRIVER: NixDriver = NixDriver::new());

impl Driver for NixDriver {
    fn hostname(&self, _namespace: u64) -> Result<String> {
        Ok(String::from(gethostname()?.into_string().map_err(
            |os_str| anyhow!("failed to parse hostname `{os_str:#?}` to UTF-8"),
        )?))
    }
    fn exec(
        &self,
        _namespace: u64,
        program_args: dusk::dusk_capnp::dusk_capnp::program_args::Client,
    ) -> Result<Box<dyn Process>> {
        Err(anyhow!("why"))
    }
}
