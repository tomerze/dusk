use std::rc::Rc;
use std::vec::Vec;

use anyhow::Result;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::program_args;
use linkme::distributed_slice;

#[derive(Copy, Clone)]
pub struct ProgramInfo {
    pub program_id: Option<u64>,
    pub name: &'static str,
    pub short_description: &'static str,
    pub long_description: &'static str,
    pub version: &'static str,
}

pub trait ProgramArgsBuilder {
    fn build(&self, client: dusk::Client, args: &str) -> Result<program_args::Client>;
}

#[derive(Clone)]
pub struct ShEntry {
    pub info: ProgramInfo,
    pub program_args_builder: Rc<dyn ProgramArgsBuilder>,
}

#[distributed_slice]
pub static SH_ENTRIES: [fn() -> ShEntry] = [..];

pub trait ShEntriesBuilder: Clone + 'static {
    fn get_entries(&self) -> Vec<ShEntry>;
}

pub trait GetAvailableProgramsInfo {
    fn get_available_programs_info(&self) -> Result<Vec<ProgramInfo>>;
}

#[derive(Clone)]
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
    fn get_available_programs_info(&self) -> Result<Vec<ProgramInfo>> {
        Ok(self.get_entries().into_iter().map(|e| e.info).collect())
    }
}
