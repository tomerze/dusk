use capnpc::CompilerCommand;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let capnp_dir = Path::new(&out_dir).join("capnp");
    std::fs::create_dir_all(capnp_dir.clone()).unwrap();

    write!(
        File::create(capnp_dir.clone().join("dusk.capnp")).unwrap(),
        "{}",
        dusk_capnp::SCHEMA
    )
    .unwrap();

    std::fs::copy(
        "/usr/local/include/capnp/stream.capnp",
        capnp_dir.join("stream.capnp"),
    )
    .unwrap();

    CompilerCommand::new()
        .src_prefix(out_dir.clone())
        .file(capnp_dir.join("stream.capnp"))
        .run()
        .unwrap();

    CompilerCommand::new()
        .import_path(out_dir)
        .crate_provides("dusk_capnp", [0xace6963097d486d6])
        .file("capnp/sh.capnp")
        .run()
        .unwrap();
}
