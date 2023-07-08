/// Dusk driver
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    fn name(&self, namespace: u64) -> String;
}

extern "Rust" {
    fn _dusk_name(namespace: u64) -> String;
}

pub fn name(namespace: u64) -> String {
    unsafe { _dusk_name(namespace) }
}

/// Set the dusk Driver implementation.
#[macro_export]
macro_rules! dusk_driver_impl {
    (static $name:ident: $t: ty = $val:expr) => {
        static $name: $t = $val;

        #[no_mangle]
        fn _dusk_name(namespace: u64) -> String {
            <$t as $crate::driver::Driver>::name(&$name, namespace)
        }
    };
}
