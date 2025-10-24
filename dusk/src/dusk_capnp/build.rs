use capnpc::CompilerCommand;
use std::path::{Path, PathBuf};
use std::process::Command;

fn ensure_capnp_compiler() -> PathBuf {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let capnp_root = Path::new(&out_dir).join("capnproto");
    let capnp_bin = capnp_root.join("bin").join("capnp");

    if capnp_bin.exists() {
        println!("cargo:warning=Using previously built capnp compiler at {}", capnp_bin.display());
        return capnp_bin;
    }

    println!("cargo:warning=Building capnp compiler from source...");

    // Copy capnproto from vendor submodule if not present
    if !capnp_root.join("c++").exists() {
        println!("cargo:warning=Copying capnproto from vendor submodule...");
        
        // Find the project root by looking for Cargo.toml
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let mut project_root = Path::new(&manifest_dir);
        
        // Navigate up to find the workspace root (where vendor/ is located)
        while !project_root.join("vendor").join("capnproto").exists() {
            if let Some(parent) = project_root.parent() {
                project_root = parent;
            } else {
                panic!("Could not find vendor/capnproto directory. Make sure the capnproto submodule is initialized.");
            }
        }
        
        let vendor_capnp = project_root.join("vendor").join("capnproto");
        
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

    println!("cargo:warning=Configuring Cap'n Proto build...");
    let status = Command::new("sh")
        .current_dir(&build_dir)
        .arg("-c")
        .arg(format!("cmake -DCMAKE_INSTALL_PREFIX={} -DCMAKE_BUILD_TYPE=Release . || (autoreconf -i && ./configure --prefix={})", prefix, prefix))
        .status()
        .expect("Failed to configure capnproto. Is cmake or autotools installed?");

    if !status.success() {
        panic!("Failed to configure capnproto build");
    }

    println!("cargo:warning=Building Cap'n Proto (this will take a while)...");
    let status = Command::new("make")
        .current_dir(&build_dir)
        .arg("-j")
        .arg(num_cpus::get().to_string())
        .status()
        .expect("Failed to build capnproto. Is make installed?");

    if !status.success() {
        panic!("Failed to build capnproto");
    }

    println!("cargo:warning=Installing Cap'n Proto to local prefix...");
    let status = Command::new("make")
        .current_dir(&build_dir)
        .arg("install")
        .status()
        .expect("Failed to install capnproto");

    if !status.success() {
        panic!("Failed to install capnproto");
    }

    println!("cargo:warning=Cap'n Proto built successfully!");
    capnp_bin
}

fn main() {
    let capnp_bin = ensure_capnp_compiler();
    
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
