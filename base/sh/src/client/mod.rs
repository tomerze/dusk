pub mod args;
pub mod cli;
pub mod prompt;
pub mod run_prompt;
pub mod shell;
pub mod stop;

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

**Usage**

* `sh <command>` — run a command, quoting it if it has spaces: `sh "ps; date"`.
* `sh -d <command>` — run it detached from the current session.
* `sh --server [<pid>]` — start the node's default shell server, or a shell
  server at `<pid>`, and leave it running for clients to attach to.
* `sh --prompt [<pid>]` — open a prompt on this terminal, attached to the
  node's default shell server or to the one at `<pid>`. `exit` closes it, and
  the shell server it was attached to keeps running.

`sh` on its own is not a command. A terminal has one prompt, so `sh --prompt`
is refused at a prompt.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(ShProgramArgsBuilder {}),
    }
}
