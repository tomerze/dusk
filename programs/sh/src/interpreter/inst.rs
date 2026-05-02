use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use dusk_capnp::dusk_capnp::program_args;

pub type Frame = Vec<Inst>;

pub struct ScriptMessage(pub capnp::message::Builder<capnp::message::HeapAllocator>);
// Safety: HeapAllocator owns its segments via Vec; the internal pointers point into
// that owned heap data, which stays valid across thread moves.
unsafe impl Send for ScriptMessage {}
unsafe impl Sync for ScriptMessage {}

pub enum Inst {
    ProgramArgs(program_args::Client),
    Call(String),
    TailCall(String),
    DefineFunction { symbol: String, body: Arc<ScriptMessage> },
    JumpIfOk(usize),
    JumpIfError(usize),
}

pub fn format_instructions(instructions: &[Inst]) -> String {
    let mut output = String::new();
    write_instructions(&mut output, instructions, 0);
    output
}

fn write_instructions(output: &mut String, instructions: &[Inst], indent: usize) {
    for (pc, instruction) in instructions.iter().enumerate() {
        let _ = write!(output, "{:indent$}{pc:04}: ", "");

        match instruction {
            Inst::ProgramArgs(_) => {
                let _ = writeln!(output, "program_args");
            }
            Inst::Call(symbol) => {
                let _ = writeln!(output, "call {symbol}");
            }
            Inst::TailCall(symbol) => {
                let _ = writeln!(output, "tail_call {symbol}");
            }
            Inst::DefineFunction { symbol, .. } => {
                let _ = writeln!(output, "define {symbol}");
            }
            Inst::JumpIfOk(target) => {
                let _ = writeln!(output, "jump_if_ok {target:04}");
            }
            Inst::JumpIfError(target) => {
                let _ = writeln!(output, "jump_if_error {target:04}");
            }
        }
    }
}
