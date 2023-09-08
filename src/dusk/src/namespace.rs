use log::info;

pub struct Namespace {
    pub id: u64,
}

impl Namespace {
    pub fn new(id: u64) -> Self {
        let id = id;
        info!("namespace `{}` created", id);
        Namespace { id }
    }
}
