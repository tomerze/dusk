use dusk_core::driver::{File, FsDriver, OpenMode, Stat};
use dusk_program::anyhow::{Context, Result, anyhow};
use std::fs::{Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

pub(crate) struct StdFsDriver;

pub(crate) struct StdFile {
    file: Mutex<std::fs::File>,
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
        mode: if metadata.permissions().readonly() {
            0o444
        } else {
            0o666
        },
        modified: metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64),
    })
}

impl StdFile {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, std::fs::File>> {
        self.file
            .lock()
            .map_err(|_| anyhow!("`{}` was poisoned by a panic", self.path))
    }
}

#[async_trait::async_trait]
impl FsDriver for StdFsDriver {
    async fn open(&self, path: &str, mode: OpenMode) -> Result<Box<dyn File>> {
        let file = OpenOptions::new()
            .read(mode.read)
            .write(mode.write)
            .create(mode.create)
            .truncate(mode.truncate)
            .open(path)
            .with_context(|| format!("couldn't open `{path}`"))?;
        Ok(Box::new(StdFile {
            file: Mutex::new(file),
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
impl File for StdFile {
    async fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let mut file = self.lock()?;
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.read(buffer))
            .with_context(|| format!("couldn't read `{}` at {offset}", self.path))
    }

    async fn write(&self, offset: u64, data: &[u8]) -> Result<usize> {
        let mut file = self.lock()?;
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.write(data))
            .with_context(|| format!("couldn't write `{}` at {offset}", self.path))
    }

    async fn stat(&self) -> Result<Stat> {
        let metadata = self.lock()?.metadata()?;
        stat(file_name(&self.path), &metadata)
    }

    async fn truncate(&self, length: u64) -> Result<()> {
        self.lock()?
            .set_len(length)
            .with_context(|| format!("couldn't truncate `{}` to {length}", self.path))
    }

    async fn sync(&self) -> Result<()> {
        self.lock()?
            .sync_all()
            .with_context(|| format!("couldn't sync `{}`", self.path))
    }
}
