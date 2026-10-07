use std::path::{Path, PathBuf};
use std::process::Command;

fn watch_schemas(directory: &Path) {
    println!("cargo:rerun-if-changed={}", directory.display());
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "capnp")
        {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn main() {
    let manifest_directory = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repository = manifest_directory.join("../../..");
    let script = manifest_directory.join("../schemas/build.sh");
    let output_directory = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("schemas");

    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-env-changed=DUSK_GIT_REV");
    watch_schemas(&repository.join("dusk/src/dusk_capnp/capnp"));
    watch_schemas(&repository.join("base/logs/capnp/otlp"));
    for program in std::fs::read_dir(repository.join("base"))
        .expect("read base/")
        .flatten()
    {
        watch_schemas(&program.path().join("capnp"));
    }

    if output_directory.exists() {
        std::fs::remove_dir_all(&output_directory).expect("clear the previous schema bundle");
    }

    let output = Command::new("sh")
        .arg(&script)
        .arg(&output_directory)
        .env("CAPNP", dusk_capnp::capnp_bin_path())
        .output()
        .expect("run services/nightfall/schemas/build.sh");
    if !output.status.success() {
        panic!(
            "services/nightfall/schemas/build.sh failed with {}:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let bundle = String::from_utf8(output.stdout).expect("bundle path is UTF-8");
    let bundle = bundle.trim();
    println!("cargo:rustc-env=NIGHTFALL_TREE_BUNDLE={bundle}");
    println!(
        "cargo:rustc-env=NIGHTFALL_TREE_SCHEMAS={}",
        output_directory.display()
    );
}
