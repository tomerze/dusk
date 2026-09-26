use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::Write;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh_compiler::BytecodeMessage;

pub type Instructions = Vec<Inst>;

pub enum Inst {
    ProgramArgs(Rc<ProgramArgs>),
    Call(String),
    TailCall(String),
    DefineFunction {
        symbol: String,
        body: Rc<RefCell<BytecodeMessage>>,
    },
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
                let _ = write!(output, "program_args; ");
            }
            Inst::Call(symbol) => {
                let _ = write!(output, "call {symbol}; ");
            }
            Inst::TailCall(symbol) => {
                let _ = write!(output, "tail_call {symbol}; ");
            }
            Inst::DefineFunction { symbol, .. } => {
                let _ = write!(output, "define {symbol}; ");
            }
            Inst::JumpIfOk(target) => {
                let _ = write!(output, "jump_if_ok {target:04}; ");
            }
            Inst::JumpIfError(target) => {
                let _ = write!(output, "jump_if_error {target:04}; ");
            }
        }
    }
}
