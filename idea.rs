pub struct PsLauncher 

pub struct PsProcess 

struct PsProgramArgsBuilder

pub struct PsArgs - args.type_name

pub struct PsPortal - by portal.type_name

---
impl PsLauncher

impl PsProcess

impl PsProgramArgsBuilder

impl PsArgs -- args.type_name

impl PsPortal - by portal.type_name

---
impl dusk_program::launcher::Launcher for PsLauncher

impl dusk_program::process::Process for PsProcess 

impl ProgramArgsBuilder for PsProgramArgsBuilder
--- portal
impl dusk_capnp::dusk_capnp::portal::Server for PsPortal 
- will be under args.server and functions [input, output] would be overriden
impl ps_capnp::ps_portal::Server for PsPortal - portal.server
--- args
impl dusk_capnp::dusk_capnp::program_args::Server for PsArgs - NOT NEEDED
impl ps_capnp::ps_args::Server for PsArgs - args.server
---
pub fn program_args_builder_entry() -> ShEntry - 

---------


#![program::definition]
{
    metadata {
        name: "ps",
        version: "0.1.0",
        program_id: ps_capnp::PROGRAM_ID,
    }
    state {
        args: {},
        portal: {},
        launcher: {},
        process: {}
    }
}

#![program::launcher::impl]
{
}

#![program::launcher::mixin]
{
}

#![program::process::impl]
{
}

#![program::process::mixin]
{
    
}

#![program::args::impl]
{
}

#![program::args::rpc]
{
    
}

#![program::portal::impl]
{
}

#![program::portal::rpc]
{
    
}

#![dusk_program_sh::sh_entry]
{

    info: {

    }

    builder_type_name: PsProgramArgsBuilder,
}
