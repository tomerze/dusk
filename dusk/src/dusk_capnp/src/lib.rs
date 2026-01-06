use capnpc::CompilerCommand;
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[allow(clippy::all)]
extern crate alloc;

pub mod prelude;

pub mod stream_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/stream_capnp.rs"));
}

#[allow(clippy::all)]
pub mod dusk_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/dusk_capnp.rs"));
}

pub static DUSK_SCHEMA: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/dusk.capnp"));

pub const fn capnp_bin_path() -> &'static str {
    match option_env!("DUSK_CAPNP_BIN_PATH") {
        Some(path) => path,
        None => panic!("DUSK_CAPNP_BIN_PATH is not set; ensure dusk_capnp's build script ran"),
    }
}

/// Like capnp_rpc's `pry!()`, but supports `no_std`
#[macro_export]
macro_rules! pry {
    ($expr:expr) => {
        match $expr {
            ::core::result::Result::Ok(val) => val,
            ::core::result::Result::Err(err) => {
                return ::capnp::capability::Promise::err(::core::convert::From::from(err))
            }
        }
    };
}

pub fn build_capnp_file(path: &str) {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let capnp_dir = Path::new(&out_dir).join("capnp");
    std::fs::create_dir_all(capnp_dir.clone()).unwrap();

    write!(
        File::create(capnp_dir.clone().join("dusk.capnp")).unwrap(),
        "{}",
        DUSK_SCHEMA
    )
    .unwrap();
    let capnp_path = Path::new(capnp_bin_path());
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
        .file(path)
        .run()
        .unwrap();
}
