use std::rc::Rc;
use std::vec::Vec;

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

pub trait ProgramArgsBuilder {
    fn build(&self, client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>>;
}

#[derive(Clone)]
pub struct ShEntry {
    pub info: EntryInfo,
    pub program_args_builder: Rc<dyn ProgramArgsBuilder>,
}

#[distributed_slice]
pub static SH_ENTRIES: [fn() -> ShEntry] = [..];

pub trait ShEntriesBuilder: Clone + 'static {
    fn get_entries(&self) -> Vec<ShEntry>;
}

pub trait GetAvailableProgramsInfo {
    fn get_available_programs_info(&self) -> anyhow::Result<Vec<EntryInfo>>;
}

#[derive(Clone, Default)]
pub struct StaticShEntriesBuilder {}

impl ShEntriesBuilder for StaticShEntriesBuilder {
    fn get_entries(&self) -> Vec<ShEntry> {
        SH_ENTRIES.iter().map(|f| f()).collect()
    }
}

#[derive(Clone)]
pub struct DynamicShEntriesBuilder {
    pub entries: Vec<ShEntry>,
}

impl ShEntriesBuilder for DynamicShEntriesBuilder {
    fn get_entries(&self) -> Vec<ShEntry> {
        self.entries.clone()
    }
}

impl<T: ShEntriesBuilder> GetAvailableProgramsInfo for T {
    fn get_available_programs_info(&self) -> anyhow::Result<Vec<EntryInfo>> {
        Ok(self.get_entries().into_iter().map(|e| e.info).collect())
    }
}
