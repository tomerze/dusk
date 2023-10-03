use anyhow::{anyhow, Result};
use dusk::driver::Driver;
use nix::unistd::gethostname;

struct NixDriver {}

dusk::dusk_driver_impl!(static DRIVER: NixDriver = NixDriver{});

impl Driver for NixDriver {
    fn hostname(&self, _namespace: u64) -> Result<String> {
        Ok(String::from(gethostname()?.into_string().map_err(
            |os_str| anyhow!("failed to parse hostname `{os_str:#?}` to UTF-8"),
        )?))
    }
}
