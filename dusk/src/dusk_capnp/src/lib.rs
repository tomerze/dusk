use capnpc::CompilerCommand;
use std::path::Path;

#[allow(clippy::all)]
extern crate alloc;

/// The version of the `dusk_capnp` crate (read from Cargo.toml at compile time).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The git revision of the workspace at build time.
pub const GIT_REV: &str = env!("GIT_REV");

// Re-export Cap'n Proto crates for all dependent crates
pub extern crate capnp;
pub extern crate capnp_rpc;
pub extern crate capnpc;

pub mod prelude;

pub mod stream_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/stream_capnp.rs"));
}

#[allow(clippy::all)]
pub mod dusk_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/dusk_capnp.rs"));
}

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
                return $crate::capnp::capability::Promise::err(::core::convert::From::from(err))
            }
        }
    };
}

pub struct CapnpDep {
    /// Path to the `.capnp` source file (relative to workspace root or absolute).
    pub schema: &'static str,
    /// The Rust crate that provides the generated code for this schema.
    pub crate_name: &'static str,
    /// The top-level schema IDs contained in this file.
    pub schema_ids: &'static [u64],
}

/// The `dusk.capnp` schema, always available as a dependency.
pub const DUSK_CAPNP: CapnpDep = CapnpDep {
    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/dusk.capnp"),
    crate_name: "dusk_capnp",
    schema_ids: &[0xace6963097d486d6],
};

/// The `stream.capnp` schema, always available as a dependency.
pub const STREAM_CAPNP: CapnpDep = CapnpDep {
    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/stream.capnp"),
    crate_name: "dusk_capnp",
    schema_ids: &[0x86c366a91393f3f8],
};

/// Compile a `.capnp` schema file into Rust code.
pub fn build_capnp(path: &str, deps: &[CapnpDep]) {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let capnp_dir = Path::new(&out_dir).join("capnp");
    std::fs::create_dir_all(&capnp_dir).unwrap();

    let capnp_path = Path::new(capnp_bin_path());
    if !capnp_path.exists() {
        panic!(
            "Expected capnp compiler built by dusk_capnp at {}",
            capnp_path.display()
        );
    }

    // Always include the core schemas
    let builtins: &[CapnpDep] = &[DUSK_CAPNP, STREAM_CAPNP];

    let mut cmd = CompilerCommand::new();
    cmd.capnp_executable(capnp_path);
    cmd.import_path(&out_dir);

    for dep in builtins.iter().chain(deps.iter()) {
        // Copy schema into the import directory so `import "/capnp/foo.capnp"` resolves
        let src = Path::new(dep.schema);
        let file_name = src
            .file_name()
            .expect("CapnpDep schema path must have a filename");
        let dest = capnp_dir.join(file_name);
        std::fs::copy(src, &dest).unwrap_or_else(|e| {
            panic!(
                "Failed to copy {} to {}: {}",
                src.display(),
                dest.display(),
                e
            )
        });

        // Register crate_provides for each schema ID
        cmd.crate_provides(dep.crate_name, dep.schema_ids.iter().copied());
    }

    cmd.file(path).run().unwrap();
}

/// Backwards-compatible wrapper — compiles a capnp file with only the default dusk deps.
pub fn build_capnp_file(path: &str) {
    build_capnp(path, &[]);
}
