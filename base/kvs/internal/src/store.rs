use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use dusk_core::driver::{File, FsDriver, OpenMode};
use dusk_program::anyhow::{self, Context as _};
use dusk_program::dusk_capnp::capnp;
use dusk_program::dusk_capnp::dusk_capnp::value;
use dusk_program::hashbrown::HashMap;
use dusk_program::value::Value;
use hkdf::Hkdf;
use nohash_hasher::BuildNoHashHasher;
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use sha2::{Digest, Sha256};

const KEY_INFO: &[u8] = b"dusk-kvs-persistent-v1";
const NONCE_INFO: &[u8] = b"dusk-kvs-persistent-nonces-v1";
const LENGTH_SIZE: usize = 4;
const NONCE_SIZE: usize = 24;
const DELETE: u8 = 0;
const SET: u8 = 1;
const READ_SIZE: usize = 64 * 1024;
const COMPACTION_SLACK: u64 = 64 * 1024;

pub(crate) enum State {
    Closed,
    Open(Box<Log>),
    Unavailable(String),
    Released,
}

impl State {
    pub(crate) fn log(&mut self) -> anyhow::Result<&mut Log> {
        match self {
            State::Open(log) => Ok(log),
            State::Unavailable(reason) => {
                anyhow::bail!("the persistent kvs file is unavailable: {reason}")
            }
            State::Closed => anyhow::bail!("the persistent kvs file is not open"),
            State::Released => {
                anyhow::bail!("the kvs launcher that named the persistent kvs file is gone")
            }
        }
    }
}

pub(crate) struct Stored {
    pub(crate) value: Value,
    pub(crate) flags: u8,
    length: u64,
}

type Entries = HashMap<u64, Stored, BuildNoHashHasher<u64>>;

type Change = Option<(Value, u8)>;

pub(crate) struct Log {
    path: String,
    file: Box<dyn File>,
    cipher: XChaCha20Poly1305,
    nonces: ChaCha20Rng,
    length: u64,
    sequence: u64,
    live: u64,
    pub(crate) entries: Entries,
}

enum Stop {
    Torn,
    Damaged,
}

impl Log {
    pub(crate) async fn open(
        file_system: &dyn FsDriver,
        path: &str,
        secret: &[u8],
        salt: Option<&[u8]>,
        nonce_seed: u64,
    ) -> anyhow::Result<Self> {
        let file = file_system
            .open(
                path,
                OpenMode {
                    read: true,
                    write: true,
                    create: true,
                    truncate: false,
                },
            )
            .await?;
        let mut bytes = Vec::new();
        let mut buffer = alloc::vec![0; READ_SIZE];
        loop {
            let read = file.read(bytes.len() as u64, &mut buffer).await?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }

        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(salt, secret)
            .expand(KEY_INFO, &mut key)
            .map_err(|_| anyhow::anyhow!("couldn't derive the persistent kvs key"))?;
        let cipher = XChaCha20Poly1305::new(&key.into());

        let mut entries = Entries::default();
        let mut live = 0;
        let mut length = 0u64;
        let mut sequence = 0;
        let mut records = 0u64;
        let mut stop = None;
        while (length as usize) < bytes.len() {
            let offset = length as usize;
            let rest = &bytes[offset..];
            let Some(size) = record_size(rest) else {
                stop = Some(Stop::Torn);
                break;
            };
            let Some(plaintext) = decrypt(&cipher, &rest[..size]) else {
                if offset == 0 {
                    anyhow::bail!(
                        "`{path}` does not open under this node's key: it was written by a node \
                         with another fleet token or device id, or its first record is damaged; \
                         it is left as it is, and persistent keys are unavailable until it is \
                         moved away and the node restarts"
                    );
                }
                stop = Some(if offset + size == bytes.len() {
                    Stop::Torn
                } else {
                    Stop::Damaged
                });
                break;
            };
            let Some((record_sequence, key, entry)) = decode(&plaintext) else {
                anyhow::bail!(
                    "`{path}` holds a record at {offset} that this node cannot read, written by \
                     another version of dusk; it is left as it is, and persistent keys are \
                     unavailable until it is moved away and the node restarts"
                );
            };
            if records > 0 && record_sequence != sequence + 1 {
                stop = Some(Stop::Damaged);
                break;
            }
            apply(&mut entries, &mut live, key, entry, size as u64);
            sequence = record_sequence;
            length += size as u64;
            records += 1;
        }

        if let Some(stop) = stop {
            let dropped = bytes.len() as u64 - length;
            match stop {
                Stop::Torn => tracing::warn!(
                    path,
                    offset = length,
                    dropped,
                    "dropped the end of the persistent kvs file: its last record runs past the end or fails to authenticate, cut short by a crash mid-write or damaged"
                ),
                Stop::Damaged => tracing::error!(
                    path,
                    offset = length,
                    dropped,
                    records,
                    "the persistent kvs file is damaged; kept the records before the damage and dropped the rest"
                ),
            }
            file.truncate(length).await?;
            file.sync().await?;
        }

        if bytes.is_empty()
            && let Err(error) = sync_directory(file_system, path).await
        {
            tracing::warn!(
                path,
                error = %format!("{error:#}"),
                "couldn't sync the directory of the new persistent kvs file; a power loss before the file system flushes it could lose the file with every key written to it"
            );
        }

        let mut seed = Sha256::new();
        seed.update(NONCE_INFO);
        seed.update(nonce_seed.to_le_bytes());
        seed.update(sequence.to_le_bytes());
        seed.update(length.to_le_bytes());
        let log = Log {
            path: String::from(path),
            file,
            cipher,
            nonces: ChaCha20Rng::from_seed(seed.finalize().into()),
            length,
            sequence,
            live,
            entries,
        };

        tracing::info!(
            path,
            records,
            keys = log.entries.len(),
            length = log.length,
            "opened the persistent kvs file"
        );
        Ok(log)
    }

    pub(crate) async fn write(&mut self, key: u64, entry: Change) -> anyhow::Result<()> {
        let record = self.encode(
            self.sequence + 1,
            key,
            entry.as_ref().map(|(value, flags)| (value, *flags)),
        )?;
        if let Err(error) = append(&*self.file, self.length, &record).await {
            if let Err(truncate_error) = self.file.truncate(self.length).await {
                tracing::warn!(
                    path = self.path.as_str(),
                    error = %format!("{truncate_error:#}"),
                    "couldn't cut a failed write off the persistent kvs file; if the record reached the disk whole, the next open replays it"
                );
            }
            return Err(error);
        }
        self.length += record.len() as u64;
        self.sequence += 1;
        apply(
            &mut self.entries,
            &mut self.live,
            key,
            entry,
            record.len() as u64,
        );
        tracing::debug!(path = self.path.as_str(), key, "kvs persistent write");
        Ok(())
    }

    pub(crate) fn needs_compaction(&self) -> bool {
        self.length > 2 * self.live + COMPACTION_SLACK
    }

    pub(crate) async fn compact(&mut self, file_system: &dyn FsDriver) {
        let from = self.length;
        match self.compacted(file_system).await {
            Ok((file, length, sequence)) => {
                self.file = file;
                self.length = length;
                self.sequence = sequence;
                self.live = length;
                tracing::info!(
                    path = self.path.as_str(),
                    from,
                    to = length,
                    "compacted the persistent kvs file"
                );
            }
            Err(error) => tracing::warn!(
                path = self.path.as_str(),
                error = %format!("{error:#}"),
                "couldn't compact the persistent kvs file; it keeps its records and grows until a compaction succeeds"
            ),
        }
    }

    async fn compacted(
        &mut self,
        file_system: &dyn FsDriver,
    ) -> anyhow::Result<(Box<dyn File>, u64, u64)> {
        let mut sequence = self.sequence;
        let mut bytes = Vec::new();
        let mut lengths = Vec::with_capacity(self.entries.len());
        let live: Vec<(u64, Value, u8)> = self
            .entries
            .iter()
            .map(|(key, stored)| (*key, stored.value.clone(), stored.flags))
            .collect();
        for (key, value, flags) in live {
            sequence += 1;
            let record = self.encode(sequence, key, Some((&value, flags)))?;
            lengths.push((key, record.len() as u64));
            bytes.extend(record);
        }
        let temporary = format!("{}.tmp", self.path);
        let file = file_system
            .open(
                &temporary,
                OpenMode {
                    read: true,
                    write: true,
                    create: true,
                    truncate: true,
                },
            )
            .await?;
        append(&*file, 0, &bytes).await?;
        file_system.rename(&temporary, &self.path).await?;
        if let Err(error) = sync_directory(file_system, &self.path).await {
            tracing::warn!(
                path = self.path.as_str(),
                error = %format!("{error:#}"),
                "couldn't sync the directory of the compacted persistent kvs file; a power loss before the file system flushes it could bring back the file from before the compaction and lose every key written since"
            );
        }
        for (key, length) in lengths {
            if let Some(stored) = self.entries.get_mut(&key) {
                stored.length = length;
            }
        }
        Ok((file, bytes.len() as u64, sequence))
    }

    fn encode(
        &mut self,
        sequence: u64,
        key: u64,
        entry: Option<(&Value, u8)>,
    ) -> anyhow::Result<Vec<u8>> {
        let mut plaintext = Vec::new();
        plaintext.extend_from_slice(&sequence.to_le_bytes());
        plaintext.extend_from_slice(&key.to_le_bytes());
        match entry {
            None => plaintext.push(DELETE),
            Some((value, flags)) => {
                plaintext.push(SET);
                plaintext.push(flags);
                let mut message = capnp::message::Builder::new_default();
                value.write_to_builder(message.init_root::<value::Builder>())?;
                plaintext.extend(capnp::serialize::write_message_to_words(&message));
            }
        }
        let mut nonce = XNonce::default();
        self.nonces.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_slice())
            .map_err(|_| anyhow::anyhow!("couldn't encrypt a persistent kvs record"))?;
        let length = u32::try_from(ciphertext.len())
            .context("a persistent kvs value is larger than a record can hold")?;
        let mut record = Vec::with_capacity(LENGTH_SIZE + NONCE_SIZE + ciphertext.len());
        record.extend_from_slice(&length.to_le_bytes());
        record.extend_from_slice(&nonce);
        record.extend(ciphertext);
        Ok(record)
    }
}

fn apply(entries: &mut Entries, live: &mut u64, key: u64, entry: Change, length: u64) {
    if let Some(previous) = entries.remove(&key) {
        *live -= previous.length;
    }
    if let Some((value, flags)) = entry {
        *live += length;
        entries.insert(
            key,
            Stored {
                value,
                flags,
                length,
            },
        );
    }
}

fn decrypt(cipher: &XChaCha20Poly1305, record: &[u8]) -> Option<Vec<u8>> {
    let nonce = XNonce::try_from(&record[LENGTH_SIZE..LENGTH_SIZE + NONCE_SIZE]).ok()?;
    cipher
        .decrypt(&nonce, &record[LENGTH_SIZE + NONCE_SIZE..])
        .ok()
}

fn decode(plaintext: &[u8]) -> Option<(u64, u64, Change)> {
    let sequence = u64::from_le_bytes(plaintext.get(0..8)?.try_into().ok()?);
    let key = u64::from_le_bytes(plaintext.get(8..16)?.try_into().ok()?);
    match *plaintext.get(16)? {
        DELETE if plaintext.len() == 17 => Some((sequence, key, None)),
        SET => {
            let flags = *plaintext.get(17)?;
            let message = capnp::serialize::read_message(
                plaintext.get(18..)?,
                capnp::message::ReaderOptions::new(),
            )
            .ok()?;
            let value = Value::from_reader(message.get_root::<value::Reader>().ok()?).ok()?;
            Some((sequence, key, Some((value, flags))))
        }
        _ => None,
    }
}

fn record_size(bytes: &[u8]) -> Option<usize> {
    let length = u32::from_le_bytes(bytes.get(..LENGTH_SIZE)?.try_into().ok()?) as usize;
    let size = LENGTH_SIZE + NONCE_SIZE + length;
    (bytes.len() >= size).then_some(size)
}

async fn sync_directory(file_system: &dyn FsDriver, path: &str) -> anyhow::Result<()> {
    file_system
        .open(
            parent(path),
            OpenMode {
                read: true,
                ..OpenMode::default()
            },
        )
        .await?
        .sync()
        .await
}

fn parent(path: &str) -> &str {
    match path.rfind(['/', '\\']) {
        Some(0) => &path[..1],
        Some(index) => &path[..index],
        None => ".",
    }
}

async fn append(file: &dyn File, mut offset: u64, mut data: &[u8]) -> anyhow::Result<()> {
    while !data.is_empty() {
        let written = file.write(offset, data).await?;
        anyhow::ensure!(written > 0, "the file system accepted none of a write");
        offset += written as u64;
        data = &data[written..];
    }
    file.sync().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::sync::Arc;
    use core::cell::RefCell;
    use dusk_core::driver::Stat;
    use dusk_program::embassy_futures::block_on;
    use dusk_program::embassy_sync::blocking_mutex::Mutex as BlockingMutex;
    use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    const PATH: &str = "kvs";
    const SECRET: &[u8] = b"fleet token";
    const SALT: &[u8] = b"device id";

    type Contents = Arc<BlockingMutex<CriticalSectionRawMutex, RefCell<Vec<u8>>>>;

    type Files = Arc<BlockingMutex<CriticalSectionRawMutex, RefCell<BTreeMap<String, Contents>>>>;

    #[derive(Clone)]
    struct MemoryFileSystem {
        files: Files,
    }

    impl Default for MemoryFileSystem {
        fn default() -> Self {
            MemoryFileSystem {
                files: Arc::new(BlockingMutex::new(RefCell::new(BTreeMap::new()))),
            }
        }
    }

    struct MemoryFile {
        contents: Contents,
    }

    impl MemoryFileSystem {
        fn bytes(&self, path: &str) -> Vec<u8> {
            self.files
                .lock(|files| files.borrow()[path].lock(|contents| contents.borrow().clone()))
        }

        fn replace(&self, path: &str, bytes: Vec<u8>) {
            self.files
                .lock(|files| files.borrow()[path].lock(|contents| *contents.borrow_mut() = bytes));
        }

        fn exists(&self, path: &str) -> bool {
            self.files.lock(|files| files.borrow().contains_key(path))
        }
    }

    #[dusk_program::async_trait::async_trait]
    impl FsDriver for MemoryFileSystem {
        async fn open(&self, path: &str, mode: OpenMode) -> anyhow::Result<Box<dyn File>> {
            self.files.lock(|files| {
                let mut files = files.borrow_mut();
                if !files.contains_key(path) {
                    anyhow::ensure!(mode.create, "no file `{path}`");
                    files.insert(
                        String::from(path),
                        Arc::new(BlockingMutex::new(RefCell::new(Vec::new()))),
                    );
                }
                let contents = files[path].clone();
                if mode.truncate {
                    contents.lock(|contents| contents.borrow_mut().clear());
                }
                Ok(Box::new(MemoryFile { contents }) as Box<dyn File>)
            })
        }

        async fn stat(&self, path: &str) -> anyhow::Result<Stat> {
            anyhow::bail!("the log never stats `{path}`")
        }

        async fn remove(&self, path: &str) -> anyhow::Result<()> {
            anyhow::bail!("the log never removes `{path}`")
        }

        async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
            self.files.lock(|files| {
                let mut files = files.borrow_mut();
                let contents = files
                    .remove(from)
                    .ok_or_else(|| anyhow::anyhow!("no file `{from}`"))?;
                files.insert(String::from(to), contents);
                Ok(())
            })
        }

        async fn create_dir(&self, path: &str) -> anyhow::Result<()> {
            anyhow::bail!("the log never creates `{path}`")
        }

        async fn read_dir(&self, path: &str) -> anyhow::Result<Vec<Stat>> {
            anyhow::bail!("the log never reads `{path}`")
        }
    }

    #[dusk_program::async_trait::async_trait]
    impl File for MemoryFile {
        async fn read(&self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<usize> {
            Ok(self.contents.lock(|contents| {
                let contents = contents.borrow();
                let start = (offset as usize).min(contents.len());
                let read = buffer.len().min(contents.len() - start);
                buffer[..read].copy_from_slice(&contents[start..start + read]);
                read
            }))
        }

        async fn write(&self, offset: u64, data: &[u8]) -> anyhow::Result<usize> {
            self.contents.lock(|contents| {
                let mut contents = contents.borrow_mut();
                let end = offset as usize + data.len();
                if contents.len() < end {
                    contents.resize(end, 0);
                }
                contents[offset as usize..end].copy_from_slice(data);
            });
            Ok(data.len())
        }

        async fn stat(&self) -> anyhow::Result<Stat> {
            anyhow::bail!("the log never stats its file")
        }

        async fn truncate(&self, length: u64) -> anyhow::Result<()> {
            self.contents
                .lock(|contents| contents.borrow_mut().resize(length as usize, 0));
            Ok(())
        }

        async fn sync(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn open(
        file_system: &MemoryFileSystem,
        secret: &[u8],
        salt: Option<&[u8]>,
    ) -> anyhow::Result<Log> {
        block_on(Log::open(file_system, PATH, secret, salt, 7))
    }

    fn text(value: &str) -> Value {
        Value::String(String::from(value))
    }

    fn stored(log: &Log, key: u64) -> Option<(Value, u8)> {
        log.entries
            .get(&key)
            .map(|stored| (stored.value.clone(), stored.flags))
    }

    #[test]
    fn a_reopened_log_holds_what_was_written() {
        let file_system = MemoryFileSystem::default();
        let mut log = open(&file_system, SECRET, Some(SALT)).unwrap();
        block_on(log.write(1, Some((text("first"), 4)))).unwrap();
        block_on(log.write(2, Some((Value::Uint(7), 6)))).unwrap();
        block_on(log.write(1, Some((text("second"), 4)))).unwrap();
        block_on(log.write(3, Some((Value::Bool(true), 4)))).unwrap();
        block_on(log.write(3, None)).unwrap();
        drop(log);

        let log = open(&file_system, SECRET, Some(SALT)).unwrap();
        assert_eq!(stored(&log, 1), Some((text("second"), 4)));
        assert_eq!(stored(&log, 2), Some((Value::Uint(7), 6)));
        assert_eq!(stored(&log, 3), None, "a deleted key stays deleted");
        assert_eq!(log.sequence, 5);
        assert_eq!(log.length, file_system.bytes(PATH).len() as u64);
    }

    #[test]
    fn the_file_holds_no_value_in_the_clear() {
        let file_system = MemoryFileSystem::default();
        let mut log = open(&file_system, SECRET, Some(SALT)).unwrap();
        block_on(log.write(1, Some((text("a value worth hiding"), 4)))).unwrap();
        let bytes = file_system.bytes(PATH);
        let value = b"a value worth hiding";
        assert!(!bytes.windows(value.len()).any(|window| window == value));
    }

    #[test]
    fn a_torn_last_record_is_dropped_and_the_log_goes_on() {
        let file_system = MemoryFileSystem::default();
        let mut log = open(&file_system, SECRET, Some(SALT)).unwrap();
        block_on(log.write(1, Some((text("kept"), 4)))).unwrap();
        let first = file_system.bytes(PATH).len();
        block_on(log.write(2, Some((text("torn"), 4)))).unwrap();
        drop(log);
        let mut bytes = file_system.bytes(PATH);
        bytes.truncate(bytes.len() - 5);
        file_system.replace(PATH, bytes);

        let mut log = open(&file_system, SECRET, Some(SALT)).unwrap();
        assert_eq!(stored(&log, 1), Some((text("kept"), 4)));
        assert_eq!(stored(&log, 2), None, "the torn record is gone");
        assert_eq!(
            file_system.bytes(PATH).len(),
            first,
            "the file is cut back to its last whole record"
        );

        block_on(log.write(2, Some((text("written again"), 4)))).unwrap();
        drop(log);
        let log = open(&file_system, SECRET, Some(SALT)).unwrap();
        assert_eq!(stored(&log, 2), Some((text("written again"), 4)));
    }

    #[test]
    fn a_torn_length_is_dropped() {
        let file_system = MemoryFileSystem::default();
        let mut log = open(&file_system, SECRET, Some(SALT)).unwrap();
        block_on(log.write(1, Some((text("kept"), 4)))).unwrap();
        drop(log);
        let mut bytes = file_system.bytes(PATH);
        let whole = bytes.len();
        bytes.extend_from_slice(&[9, 0]);
        file_system.replace(PATH, bytes);

        let log = open(&file_system, SECRET, Some(SALT)).unwrap();
        assert_eq!(stored(&log, 1), Some((text("kept"), 4)));
        assert_eq!(file_system.bytes(PATH).len(), whole);
    }

    #[test]
    fn damage_keeps_the_records_before_it() {
        let file_system = MemoryFileSystem::default();
        let mut log = open(&file_system, SECRET, Some(SALT)).unwrap();
        block_on(log.write(1, Some((text("before"), 4)))).unwrap();
        let first = file_system.bytes(PATH).len();
        block_on(log.write(2, Some((text("damaged"), 4)))).unwrap();
        block_on(log.write(3, Some((text("after"), 4)))).unwrap();
        drop(log);
        let mut bytes = file_system.bytes(PATH);
        bytes[first + LENGTH_SIZE + NONCE_SIZE + 2] ^= 0xff;
        file_system.replace(PATH, bytes);

        let log = open(&file_system, SECRET, Some(SALT)).unwrap();
        assert_eq!(stored(&log, 1), Some((text("before"), 4)));
        assert_eq!(stored(&log, 2), None);
        assert_eq!(stored(&log, 3), None);
        assert_eq!(file_system.bytes(PATH).len(), first);
    }

    #[test]
    fn another_key_refuses_the_file_and_leaves_it_alone() {
        let file_system = MemoryFileSystem::default();
        let mut log = open(&file_system, SECRET, Some(SALT)).unwrap();
        block_on(log.write(1, Some((text("secret"), 4)))).unwrap();
        drop(log);
        let bytes = file_system.bytes(PATH);

        for (secret, salt) in [
            (b"another fleet token".as_slice(), Some(SALT)),
            (SECRET, Some(b"another device id".as_slice())),
            (SECRET, None),
        ] {
            let Err(error) = open(&file_system, secret, salt) else {
                panic!("a log written under another key opened");
            };
            assert!(
                format!("{error:#}").contains("does not open under this node's key"),
                "unexpected error: {error:#}"
            );
            assert_eq!(
                file_system.bytes(PATH),
                bytes,
                "a refused file is left as it is"
            );
        }
    }

    #[test]
    fn an_empty_file_opens_empty() {
        let file_system = MemoryFileSystem::default();
        let log = open(&file_system, SECRET, None).unwrap();
        assert!(log.entries.is_empty());
        assert_eq!(log.length, 0);
        assert!(file_system.exists(PATH), "opening creates the file");
    }

    #[test]
    fn compaction_keeps_the_live_records_only() {
        let file_system = MemoryFileSystem::default();
        let mut log = open(&file_system, SECRET, Some(SALT)).unwrap();
        let large = Value::Bytes(alloc::vec![7; 4096]);
        block_on(log.write(2, Some((text("untouched"), 6)))).unwrap();
        let mut compacted = false;
        for round in 0..64u64 {
            block_on(log.write(1, Some((large.clone(), 4)))).unwrap();
            block_on(log.write(3, Some((Value::Uint(round), 4)))).unwrap();
            let before = log.length;
            if log.needs_compaction() {
                block_on(log.compact(&file_system));
            }
            if log.length < before {
                compacted = true;
                assert_eq!(
                    log.length, log.live,
                    "a compacted file holds live records only"
                );
                assert_eq!(log.length, file_system.bytes(PATH).len() as u64);
            }
        }
        assert!(
            compacted,
            "64 rewrites of a 4 KiB value never compacted the log"
        );
        assert!(
            !file_system.exists("kvs.tmp"),
            "the compacted file replaced the log"
        );
        let sequence = log.sequence;
        block_on(log.write(4, Some((text("after compaction"), 4)))).unwrap();
        drop(log);

        let log = open(&file_system, SECRET, Some(SALT)).unwrap();
        assert_eq!(stored(&log, 1), Some((large, 4)));
        assert_eq!(stored(&log, 2), Some((text("untouched"), 6)));
        assert_eq!(stored(&log, 3), Some((Value::Uint(63), 4)));
        assert_eq!(stored(&log, 4), Some((text("after compaction"), 4)));
        assert_eq!(log.sequence, sequence + 1);
    }
}
