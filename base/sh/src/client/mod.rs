pub mod args;
pub mod cli;
pub mod prompt;
pub mod run_prompt;
pub mod shell;

use crate::VERSION;
use crate::entry::{EntryInfo, ShEntry};
use crate::sh_capnp;
use cli::ShProgramArgsBuilder;
use std::rc::Rc;

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(sh_capnp::PROGRAM_ID),
            name: "sh",
            short_description: "run Dusk shell commands",
            long_description: r#"
`sh` runs commands in the Dusk shell — Dusk's own shell language, not a Unix
shell. Dusk shell commands run Dusk programs built into the Dusk Node.

* Use `sh <command>` (or `sh "<command>"`) to run a command.
* Use `sh -d <command>` to run it detached from the current session.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(ShProgramArgsBuilder {}),
    }
}
