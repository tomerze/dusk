use alloc::string::String;

#[derive(Clone, Debug, Default)]
pub struct KvsConfig {
    pub persistent: Option<String>,
}
