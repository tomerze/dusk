use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use dusk_program::anyhow::Result;

/// Dusk driver.
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    fn hostname(&self) -> Result<String>;

    fn tid(&self) -> u64;

    fn fs_driver(&self) -> Result<Box<dyn FsDriver>>;

    fn exit(&self, exit_code: i32);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenMode {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    pub name: String,
    pub length: u64,
    pub is_directory: bool,
    pub mode: u32,
    pub modified: u64,
}

#[async_trait::async_trait]
pub trait FsDriver: Send + Sync {
    async fn open(&self, path: &str, mode: OpenMode) -> Result<Box<dyn File>>;

    async fn stat(&self, path: &str) -> Result<Stat>;

    async fn remove(&self, path: &str) -> Result<()>;

    async fn rename(&self, from: &str, to: &str) -> Result<()>;

    async fn create_dir(&self, path: &str) -> Result<()>;

    async fn read_dir(&self, path: &str) -> Result<Vec<Stat>>;
}

#[async_trait::async_trait]
pub trait File: Send + Sync {
    async fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<usize>;

    async fn write(&self, offset: u64, data: &[u8]) -> Result<usize>;

    async fn stat(&self) -> Result<Stat>;

    async fn truncate(&self, length: u64) -> Result<()>;

    async fn sync(&self) -> Result<()>;
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
        fn _dusk_tid() -> u64 {
            <$t as $crate::driver::Driver>::tid(&$name)
        }

        #[unsafe(no_mangle)]
        fn _dusk_fs_driver() -> Result<Box<dyn $crate::driver::FsDriver>> {
            <$t as $crate::driver::Driver>::fs_driver(&$name)
        }

        #[unsafe(no_mangle)]
        fn _dusk_exit(exit_code: i32) {
            <$t as $crate::driver::Driver>::exit(&$name, exit_code)
        }
    };
}

unsafe extern "Rust" {
    fn _dusk_hostname() -> Result<String>;

    fn _dusk_tid() -> u64;

    fn _dusk_fs_driver() -> Result<Box<dyn FsDriver>>;

    fn _dusk_exit(exit_code: i32);
}

pub fn hostname() -> Result<String> {
    unsafe { _dusk_hostname() }
}

pub fn tid() -> u64 {
    unsafe { _dusk_tid() }
}

pub fn fs_driver() -> Result<Box<dyn FsDriver>> {
    unsafe { _dusk_fs_driver() }
}

pub fn exit(exit_code: i32) {
    unsafe { _dusk_exit(exit_code) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuskImplExit {
    Code(i32),
    Panic,
}
