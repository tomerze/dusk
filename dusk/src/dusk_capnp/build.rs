use capnpc::CompilerCommand;
use std::path::{Path, PathBuf};
use std::process::Command;

fn export_capnp_env(capnp_bin: &Path) {
    println!(
        "cargo:rustc-env=DUSK_CAPNP_BIN_PATH={}",
        capnp_bin.display()
    );
}

/// Copies vendor capnproto to the destination directory.
fn copy_vendor_capnp(capnp_root: &Path) {
    // Remove any existing build to ensure clean state
    if capnp_root.exists() {
        std::fs::remove_dir_all(capnp_root).expect("Failed to remove existing capnproto directory");
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let vendor_capnp = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("vendor")
        .join("capnproto");

    if !vendor_capnp.exists() {
        panic!(
            "Capnproto submodule not found at {}. Run 'git submodule update --init --recursive'",
            vendor_capnp.display()
        );
    }

    let vendor = vendor_capnp
        .to_str()
        .expect("vendor/capnproto path contains invalid UTF-8");
    let dest = capnp_root
        .to_str()
        .expect("OUT_DIR path contains invalid UTF-8");

    let status = Command::new("cp")
        .args(["-rH", vendor, dest])
        .status()
        .expect("Failed to copy capnproto from vendor directory");

    if !status.success() {
        panic!("Failed to copy capnproto from vendor directory");
    }
}

/// Builds and installs local capnp compiler if not already present.
fn ensure_capnp_build(capnp_root: &Path) -> PathBuf {
    let capnp_bin = capnp_root.join("bin").join("capnp");

    if capnp_bin.exists() {
        println!(
            "cargo:info=Using previously built capnp compiler at {}",
            capnp_bin.display()
        );
        export_capnp_env(&capnp_bin);
        return capnp_bin;
    }

    println!("cargo:info=Building capnp compiler from source");

    copy_vendor_capnp(capnp_root);

    let build_dir = capnp_root.join("c++");
    let prefix = capnp_root
        .to_str()
        .expect("OUT_DIR path contains invalid UTF-8");

    let command = format!(
        "cmake -G 'Unix Makefiles' -DCMAKE_INSTALL_PREFIX={} -DCMAKE_BUILD_TYPE=Release . || (autoreconf -i && ./configure --prefix={})",
        prefix, prefix
    );
    let output = Command::new("sh")
        .current_dir(&build_dir)
        .arg("-c")
        .arg(&command)
        .output()
        .expect("Failed to configure Cap'n Proto. Are `cmake` and `autoconf` installed? Is the capnp submodule cloned?");

    if !output.status.success() {
        eprintln!(
            "Command failed: `{}`, exit status: {}",
            command, output.status
        );
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("Failed to configure Cap'n Proto build");
    }

    let jobs = std::env::var("NUM_JOBS").unwrap_or_else(|_| "1".into());
    let output = Command::new("make")
        .current_dir(&build_dir)
        .arg("-j")
        .arg(&jobs)
        .output()
        .expect("Failed to build capnproto. Is `make` installed?");

    if !output.status.success() {
        eprintln!(
            "Command failed: `make -j {}`, exit status: {}",
            jobs, output.status
        );
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
        eprintln!(
            "Command failed: `make install`, exit status: {}",
            output.status
        );
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("Failed to install capnproto");
    }

    export_capnp_env(&capnp_bin);
    capnp_bin
}

include!("../dusk_build/src/revision.rs");

fn emit_git_rev() {
    println!("cargo:rerun-if-env-changed=DUSK_GIT_REV");
    match std::env::var("DUSK_GIT_REV") {
        Ok(revision) if !revision.is_empty() => {
            let Some(short) = short_revision(&revision) else {
                fail_without_revision(&format!("DUSK_GIT_REV is `{revision}`"));
            };
            println!("cargo:rustc-env=GIT_REV={short}");
            return;
        }
        Ok(_) | Err(std::env::VarError::NotPresent) => {}
        Err(std::env::VarError::NotUnicode(_)) => {
            fail_without_revision("DUSK_GIT_REV is not UTF-8");
        }
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .expect("failed to execute git")
    };

    let output = match Command::new("git").args(["rev-parse", "HEAD"]).output() {
        Ok(output) if output.status.success() => output,
        Ok(output) => fail_without_revision(&format!(
            "`git rev-parse HEAD` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => fail_without_revision(&format!("couldn't run git: {error}")),
    };
    let revision = String::from_utf8_lossy(&output.stdout);
    let Some(short) = short_revision(revision.trim()) else {
        fail_without_revision(&format!(
            "`git rev-parse HEAD` printed `{}`",
            revision.trim()
        ));
    };
    println!("cargo:rustc-env=GIT_REV={short}");

    let git_dir = String::from_utf8(git(&["rev-parse", "--absolute-git-dir"]).stdout).unwrap();
    let git_dir = git_dir.trim();
    println!("cargo:rerun-if-changed={git_dir}/HEAD");
    let head_ref = git(&["symbolic-ref", "--quiet", "HEAD"]);
    if head_ref.status.success() {
        let head_ref = String::from_utf8(head_ref.stdout).unwrap();
        let ref_path = format!("{}/{}", git_dir, head_ref.trim());
        if Path::new(&ref_path).exists() {
            println!("cargo:rerun-if-changed={ref_path}");
        }
    }
}

fn main() {
    emit_git_rev();

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let capnp_root = Path::new(&out_dir).join("capnproto");
    let capnp_bin = ensure_capnp_build(&capnp_root);

    println!("cargo:rerun-if-changed=capnp/dusk.capnp");
    println!("cargo:rerun-if-changed=capnp/stream.capnp");

    let capnp_dir = Path::new(&out_dir).join("capnp");
    std::fs::create_dir_all(&capnp_dir).unwrap();

    let stream_capnp_path = capnp_dir.join("stream.capnp");
    std::fs::copy("capnp/stream.capnp", &stream_capnp_path).expect("Failed to copy stream.capnp");

    CompilerCommand::new()
        .capnp_executable(&capnp_bin)
        .src_prefix(out_dir.clone())
        .file(stream_capnp_path)
        .run()
        .unwrap();

    CompilerCommand::new()
        .capnp_executable(&capnp_bin)
        .file("capnp/dusk.capnp")
        .run()
        .unwrap();
}
