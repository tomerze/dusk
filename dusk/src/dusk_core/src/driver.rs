use alloc::rc::Rc;
use alloc::string::String;
use dusk_program::anyhow::Result;
use dusk_program::launcher_set::LauncherSet;
use dusk_program::namespace::Namespace;

/// Dusk driver.
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    fn hostname(&self) -> Result<String>;

    fn launchers(&self, namespace: Rc<Namespace>) -> Result<LauncherSet>;
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
        fn _dusk_launchers(namespace: Rc<Namespace>) -> Result<LauncherSet> {
            <$t as $crate::driver::Driver>::launchers(&$name, namespace)
        }
    };
}

unsafe extern "Rust" {
    fn _dusk_hostname() -> Result<String>;

    fn _dusk_launchers(namespace: Rc<Namespace>) -> Result<LauncherSet>;
}

pub fn hostname() -> Result<String> {
    unsafe { _dusk_hostname() }
}

pub fn launchers(namespace: Rc<Namespace>) -> Result<LauncherSet> {
    unsafe { _dusk_launchers(namespace) }
}
