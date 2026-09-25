use std::rc::Rc;

use crate::linkme::distributed_slice;
use dusk_capnp::dusk_capnp::dusk;
use dusk_program::anyhow;
use dusk_program::program_args::ProgramArgs;

#[derive(Copy, Clone)]
pub struct EntryInfo {
    pub program_id: Option<u64>,
    pub name: &'static str,
    pub short_description: &'static str,
    pub long_description: &'static str,
    pub version: &'static str,
}

#[dusk_program::async_trait::async_trait(?Send)]
pub trait ProgramArgsBuilder {
    async fn build(&self, client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>>;
}

#[derive(Clone)]
pub struct ShEntry {
    pub info: EntryInfo,
    pub program_args_builder: Rc<dyn ProgramArgsBuilder>,
}

#[distributed_slice]
pub static SH_ENTRIES: [fn() -> ShEntry] = [..];

std::thread_local! {
    static ENTRIES: Rc<[ShEntry]> = SH_ENTRIES.iter().map(|entry| entry()).collect();
}

pub fn sh_entries() -> Rc<[ShEntry]> {
    ENTRIES.with(Rc::clone)
}
