use dusk_core::driver::{File, FsDriver, OpenMode, Stat};
use dusk_program::anyhow::{Context, Result};
use std::fs::{Metadata, OpenOptions};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::Path;
use std::time::UNIX_EPOCH;

pub(crate) struct NixFsDriver;

pub(crate) struct NixFile {
    file: std::fs::File,
    path: String,
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn stat(name: String, metadata: &Metadata) -> Result<Stat> {
    Ok(Stat {
        name,
        length: metadata.len(),
        is_directory: metadata.is_dir(),
        mode: metadata.mode() & 0o7777,
        modified: metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64),
    })
}

#[async_trait::async_trait]
impl FsDriver for NixFsDriver {
    async fn open(&self, path: &str, mode: OpenMode) -> Result<Box<dyn File>> {
        let file = OpenOptions::new()
            .read(mode.read)
            .write(mode.write)
            .create(mode.create)
            .truncate(mode.truncate)
            .open(path)
            .with_context(|| format!("couldn't open `{path}`"))?;
        Ok(Box::new(NixFile {
            file,
            path: path.to_string(),
        }))
    }

    async fn stat(&self, path: &str) -> Result<Stat> {
        let metadata =
            std::fs::metadata(path).with_context(|| format!("couldn't stat `{path}`"))?;
        stat(file_name(path), &metadata)
    }

    async fn remove(&self, path: &str) -> Result<()> {
        let metadata =
            std::fs::symlink_metadata(path).with_context(|| format!("couldn't stat `{path}`"))?;
        if metadata.is_dir() {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        }
        .with_context(|| format!("couldn't remove `{path}`"))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        std::fs::rename(from, to).with_context(|| format!("couldn't rename `{from}` to `{to}`"))
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        std::fs::create_dir(path).with_context(|| format!("couldn't create directory `{path}`"))
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<Stat>> {
        std::fs::read_dir(path)
            .with_context(|| format!("couldn't read directory `{path}`"))?
            .map(|entry| {
                let entry = entry.with_context(|| format!("couldn't read directory `{path}`"))?;
                stat(
                    entry.file_name().to_string_lossy().into_owned(),
                    &entry.metadata()?,
                )
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl File for NixFile {
    async fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        self.file
            .read_at(buffer, offset)
            .with_context(|| format!("couldn't read `{}` at {offset}", self.path))
    }

    async fn write(&self, offset: u64, data: &[u8]) -> Result<usize> {
        self.file
            .write_at(data, offset)
            .with_context(|| format!("couldn't write `{}` at {offset}", self.path))
    }

    async fn stat(&self) -> Result<Stat> {
        stat(file_name(&self.path), &self.file.metadata()?)
    }

    async fn truncate(&self, length: u64) -> Result<()> {
        self.file
            .set_len(length)
            .with_context(|| format!("couldn't truncate `{}` to {length}", self.path))
    }

    async fn sync(&self) -> Result<()> {
        self.file
            .sync_all()
            .with_context(|| format!("couldn't sync `{}`", self.path))
    }
}
