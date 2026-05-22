use anyhow::Result;

fn main() -> Result<()> {
    dusk_base::link_anchors();
    dusk_cli::main()
}
