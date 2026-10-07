use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdentity {
    pub device_id: String,
    pub installation_id: String,
    pub namespace_id: u64,
    pub instance: String,
    pub quarantined: bool,
}

pub trait NodeLink {
    fn epoch(&self) -> u64;
    fn identity(&self) -> &SessionIdentity;
    fn fresh_dusk(&self) -> Promise<dusk::Client, capnp::Error>;
    fn closed(&self) -> bool;
}
