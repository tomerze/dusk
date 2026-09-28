use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const INCLUDE_DIRECTORY_VARIABLE: &str = "DUSK_NODE_INCLUDE_DIRECTORY";

fn main() {
    println!("cargo::rerun-if-env-changed={INCLUDE_DIRECTORY_VARIABLE}");
    let Some(include_directory) = env::var_os(INCLUDE_DIRECTORY_VARIABLE) else {
        return;
    };
    let include_directory = PathBuf::from(include_directory);

    let mut program_header_directories: Vec<PathBuf> = env::vars_os()
        .filter_map(|(name, value)| {
            let name = name.into_string().ok()?;
            (name.starts_with("DEP_") && name.ends_with("_C_API_INCLUDE"))
                .then(|| PathBuf::from(value))
        })
        .collect();
    program_header_directories.sort();

    let mut copied = BTreeMap::new();
    for header_directory in
        std::iter::once(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/include")))
            .chain(program_header_directories)
    {
        println!("cargo::rerun-if-changed={}", header_directory.display());
        copy_headers(
            &header_directory,
            &header_directory,
            &include_directory,
            &mut copied,
        );
    }
}

fn copy_headers(
    root: &Path,
    directory: &Path,
    include_directory: &Path,
    copied: &mut BTreeMap<PathBuf, PathBuf>,
) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("couldn't read {}: {error}", directory.display()));
    for entry in entries {
        let source = entry
            .unwrap_or_else(|error| panic!("couldn't read {}: {error}", directory.display()))
            .path();
        if source.is_dir() {
            copy_headers(root, &source, include_directory, copied);
            continue;
        }
        let relative = source
            .strip_prefix(root)
            .expect("read_dir yields paths under the directory it reads")
            .to_path_buf();
        if let Some(previous) = copied.insert(relative.clone(), source.clone()) {
            panic!(
                "{} and {} are both {}; two headers cannot be exported at the same path",
                previous.display(),
                source.display(),
                relative.display()
            );
        }
        let destination = include_directory.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("couldn't create {}: {error}", parent.display()));
        }
        fs::copy(&source, &destination).unwrap_or_else(|error| {
            panic!(
                "couldn't copy {} to {}: {error}",
                source.display(),
                destination.display()
            )
        });
    }
}
