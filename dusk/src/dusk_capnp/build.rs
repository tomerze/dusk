use capnpc::CompilerCommand;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Builds and installs local capnp compiler if not already present
fn ensure_capnp_build(capnp_root: PathBuf) {
    let capnp_bin = capnp_root.join("bin").join("capnp");

    if capnp_bin.exists() {
        println!("cargo:warning=Using previously built capnp compiler at {}", capnp_bin.display());
        return;
    }

    println!("cargo:warning=Building capnp compiler from source");

    // Copy capnproto from vendor submodule if not present
    if !capnp_root.join("c++").exists() {
        
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let project_root = Path::new(&manifest_dir);

        let vendor_capnp = project_root.parent().unwrap().parent().unwrap().parent().unwrap().join("vendor").join("capnproto");

        if !vendor_capnp.exists() {
            panic!("Capnproto submodule not found at {}. Run 'git submodule update --init --recursive'", vendor_capnp.display());
        }
        
        // Copy the entire capnproto directory
        let status = Command::new("cp")
            .args(&["-r", vendor_capnp.to_str().unwrap(), capnp_root.to_str().unwrap()])
            .status()
            .expect("Failed to copy capnproto from vendor directory");
            
        if !status.success() {
            panic!("Failed to copy capnproto from vendor directory");
        }
    }

    // Build capnproto
    let build_dir = capnp_root.join("c++");
    let prefix = capnp_root.to_str().unwrap();

    let command = format!("cmake -DCMAKE_INSTALL_PREFIX={} -DCMAKE_BUILD_TYPE=Release . || (autoreconf -i && ./configure --prefix={})", prefix, prefix);
    let output = Command::new("sh")
        .current_dir(&build_dir)
        .arg("-c")
        .arg(command.clone())
        .output()
        .expect("Failed to configure Cap'n Proto. Are `cmake` and `autotools` installed?");

    if !output.status.success() {
        eprintln!("Command failed: `{}`, exit status: {}", command, output.status);
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("Failed to configure Cap'n Proto build");
    }

    let output = Command::new("make")
        .current_dir(&build_dir)
        .arg("-j")
        .arg(std::env::var("NUM_JOBS").unwrap())
        .output()
        .expect("Failed to build capnproto. Is `make` installed?");

    if !output.status.success() {
        eprintln!("Command failed: `make -j {}`, exit status: {}", std::env::var("NUM_JOBS").unwrap(), output.status);
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("Failed to build capnproto");
    }

    let output = Command::new("make")
        .current_dir(&build_dir)
        .arg("install")
        .output()
        .expect("Failed to install capnproto");

    if !output.status.success() {
        eprintln!("Command failed: `make install`, exit status: {}", output.status);
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("Failed to install capnproto");
    }
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let capnp_root = Path::new(&out_dir).join("capnproto");
    let capnp_bin = capnp_root.join("bin").join("capnp");

    ensure_capnp_build(capnp_root.clone());
    
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let capnp_dir = Path::new(&out_dir).join("capnp");
    std::fs::create_dir_all(capnp_dir.clone()).unwrap();

    // Copy vendored stream.capnp
    let stream_capnp_path = capnp_dir.join("stream.capnp");
    std::fs::copy("capnp/stream.capnp", &stream_capnp_path)
        .expect("Failed to copy stream.capnp");

    // Compile stream.capnp
    CompilerCommand::new()
        .capnp_executable(capnp_bin.clone())
        .src_prefix(out_dir.clone())
        .file(stream_capnp_path)
        .run()
        .unwrap();

    // Compile dusk.capnp
    CompilerCommand::new()
        .capnp_executable(capnp_bin)
        .file("capnp/dusk.capnp")
        .run()
        .unwrap();
}
