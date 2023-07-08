use dusk::driver::Driver;

struct LinuxDriver {}

dusk::dusk_driver_impl!(static DRIVER: LinuxDriver = LinuxDriver{});

impl Driver for LinuxDriver {
    fn name(&self, _namespace: u64) -> String {
        String::from("linux")
    }
}
