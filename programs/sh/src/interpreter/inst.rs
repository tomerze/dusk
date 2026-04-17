use dusk_capnp::dusk_capnp::program_args;

use super::execution::Mode;

pub enum Inst {
    Command {
        program_args: program_args::Client,
        mode: Mode,
    },
    JumpIfOk(usize),
    JumpIfError(usize),
}
