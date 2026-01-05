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
        dusk_capnp::DUSK_SCHEMA
    )
    .unwrap();
    let capnp_path = Path::new(dusk_capnp::capnp_bin_path());
    if !capnp_path.exists() {
        panic!(
            "Expected capnp compiler built by dusk_capnp at {}",
            capnp_path.display()
        );
    }

    let mut cmd = CompilerCommand::new();
    cmd.capnp_executable(capnp_path);

    cmd.import_path(out_dir)
        .crate_provides("dusk_capnp", [0x86c366a91393f3f8]) // stream.capnp
        .crate_provides("dusk_capnp", [0xace6963097d486d6]) // dusk.capnp
        .file("capnp/init.capnp")
        .run()
        .unwrap();
}
