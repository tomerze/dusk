use anyhow::Result;
use std::hint::black_box;

fn main() -> Result<()> {
    // Unfortunately we need to trick the linker into including all
    // crates that register program args builders

    black_box(dusk_program_ps::program_args_builder_entry);
    dusk_cli::main()
}
