use std::prelude::rust_2024::*;

use dusk_core::driver::OpenMode;

const READ_CHUNK_BYTES: usize = 8 * 1024;

pub(crate) async fn read(path: &str, limit: usize) -> anyhow::Result<Vec<u8>> {
    let file = dusk_core::driver::fs_driver()?
        .open(
            path,
            OpenMode {
                read: true,
                ..OpenMode::default()
            },
        )
        .await?;
    let mut contents = Vec::new();
    let mut buffer = vec![0u8; READ_CHUNK_BYTES];
    loop {
        let offset = u64::try_from(contents.len())?;
        let read = file.read(offset, &mut buffer).await?;
        if read == 0 {
            return Ok(contents);
        }
        contents.extend_from_slice(&buffer[..read]);
        anyhow::ensure!(
            contents.len() <= limit,
            "`{path}` is larger than {limit} bytes"
        );
    }
}
