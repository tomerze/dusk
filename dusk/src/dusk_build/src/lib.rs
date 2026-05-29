use std::path::Path;
use std::process::Command;

pub use dusk_capnp::CapnpDep;

/// Run a program crate's full build: emit the git revision, then compile its
/// Cap'n Proto schema. A program's `build.rs` should be a single call to this.
///
/// `capnp_file` is the path to the program's `.capnp` schema (relative to the
/// crate root); `deps` lists any additional schemas it imports.
pub fn build(capnp_file: &str, deps: &[CapnpDep]) {
    emit_git_rev();
    dusk_capnp::build_capnp(capnp_file, deps);
}

/// Emit the workspace git revision as a `GIT_REV` rustc env var.
///
/// Makes the first 16 hex digits of `git rev-parse HEAD` available via
/// `env!("GIT_REV")` in the calling crate. Also registers rerun-if-changed
/// hooks so the value refreshes when the checked-out commit changes.
pub fn emit_git_rev() {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .expect("failed to execute git")
    };

    let git_hash = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout).unwrap();
    println!("cargo:rustc-env=GIT_REV={}", &git_hash.trim()[..16]);

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
