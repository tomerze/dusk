use std::path::Path;
use std::process::Command;

use capnpc::CompilerCommand;
pub use dusk_capnp::CapnpDep;
use dusk_capnp::{DUSK_CAPNP, STREAM_CAPNP, capnp_bin_path};

mod revision;

use revision::{fail_without_revision, short_revision};

/// Run a program crate's full build: emit the git revision, then compile each
/// of its Cap'n Proto schemas. A program's `build.rs` should be a single call
/// to this.
///
/// Each entry is a `(file, deps)` pair: `file` is the path to a `.capnp` schema
/// (relative to the crate root) and `deps` lists the schemas it imports.
pub fn build(schemas: &[(&str, &[CapnpDep])]) {
    emit_git_rev();
    for &(file, deps) in schemas {
        build_capnp(file, deps);
    }
}

/// Emit the workspace git revision as a `GIT_REV` rustc env var.
///
/// Makes the first 16 hex digits of `git rev-parse HEAD` available via
/// `env!("GIT_REV")` in the calling crate. Also registers rerun-if-changed
/// hooks so the value refreshes when the checked-out commit changes.
pub fn emit_git_rev() {
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

    // Rebuild when the checked-out commit changes: watch HEAD (catches branch
    // switches) and the ref file it points to (catches new commits on the
    // current branch). Paths are absolute so they resolve regardless of how
    // deep in the workspace the calling crate sits.
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

/// Compile a `.capnp` schema file into Rust code.
pub fn build_capnp(path: &str, deps: &[CapnpDep]) {
    println!("cargo:rerun-if-changed={path}");

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
        println!("cargo:rerun-if-changed={}", dep.schema);

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

/// Backwards-compatible wrapper - compiles a capnp file with only the default dusk deps.
pub fn build_capnp_file(path: &str) {
    build_capnp(path, &[]);
}
