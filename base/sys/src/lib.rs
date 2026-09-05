#![allow(internal_features)]
#![feature(prelude_import)]

extern crate alloc;
extern crate capnp;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("sys", VERSION, sys_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn new(command: &str) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            root.set_command(command);
        }
        Args { data }
    }
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}
