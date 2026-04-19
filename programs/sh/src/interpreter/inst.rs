use dusk_capnp::dusk_capnp::{program_args, stream};

pub enum Inst {
    Command {
        program_args: program_args::Client,
        output: stream::Client,
    },
    JumpIfOk(usize),
    JumpIfError(usize),
}
