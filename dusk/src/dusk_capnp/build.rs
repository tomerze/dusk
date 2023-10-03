use capnpc::CompilerCommand;
use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let capnp_dir = Path::new(&out_dir).join("capnp");
    std::fs::create_dir_all(capnp_dir.clone()).unwrap();

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
        .file("capnp/dusk.capnp")
        .run()
        .unwrap();
}
