use anyhow::Result;
use std::hint::black_box;

fn main() -> Result<()> {
    // Unfortunately we need to trick the linker into including all
    // crates that register sh entries.

    black_box(dusk_program_ps::client::sh_entry);
    black_box(dusk_program_kill::client::sh_entry);
    black_box(dusk_program_true::client::sh_entry);
    black_box(dusk_program_false::client::sh_entry);
    black_box(dusk_program_sleep::client::sh_entry);
    dusk_cli::main()
}
