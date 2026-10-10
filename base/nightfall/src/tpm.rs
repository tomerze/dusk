use std::prelude::rust_2024::*;

use std::io::{ErrorKind, Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use tpm2_protocol::TpmWriter;
use tpm2_protocol::data::{TpmRc, TpmRcBase, TpmSt, TpmsAuthCommand};
use tpm2_protocol::frame::{TpmFrame, TpmResponse, TpmUnmarshalBody, tpm_marshal_command};

const MAXIMUM_FRAME_BYTES: usize = 4096;
const HEADER_BYTES: usize = 10;
const MAXIMUM_ATTEMPTS: u32 = 10;
const RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(20);

enum Channel {
    Device(std::fs::File),
    #[cfg(unix)]
    Socket(std::os::unix::net::UnixStream),
}

pub(crate) struct Tpm {
    pub(crate) path: String,
    channel: Mutex<Channel>,
}

#[derive(Debug)]
pub(crate) struct ResponseCode {
    pub(crate) command: &'static str,
    pub(crate) code: u32,
}

impl ResponseCode {
    fn is(&self, base: TpmRcBase) -> bool {
        TpmRc::try_from(self.code).is_ok_and(|code| code.base() == base)
    }

    fn asks_to_retry(&self) -> bool {
        [TpmRcBase::Retry, TpmRcBase::Yielded, TpmRcBase::Testing]
            .into_iter()
            .any(|base| self.is(base))
    }
}

impl core::fmt::Display for ResponseCode {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "the TPM refused {} with response code {:#x}",
            self.command, self.code
        )?;
        match TpmRc::try_from(self.code) {
            Ok(code) => write!(formatter, " ({code})"),
            Err(_) => Ok(()),
        }
    }
}

impl core::error::Error for ResponseCode {}

fn exchange(channel: &mut (impl Read + Write), command: &[u8]) -> anyhow::Result<Vec<u8>> {
    channel
        .write_all(command)
        .context("couldn't send a command to the TPM")?;
    let mut response = vec![0u8; MAXIMUM_FRAME_BYTES];
    let mut filled = 0;
    loop {
        let read = match channel.read(&mut response[filled..]) {
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("couldn't read the TPM's response"),
        };
        anyhow::ensure!(
            read != 0,
            "the TPM closed the connection after {filled} bytes of a response"
        );
        filled += read;
        if filled < HEADER_BYTES {
            continue;
        }
        let size = usize::try_from(u32::from_be_bytes([
            response[2],
            response[3],
            response[4],
            response[5],
        ]))?;
        anyhow::ensure!(
            (HEADER_BYTES..=MAXIMUM_FRAME_BYTES).contains(&size) && filled <= size,
            "the TPM sent {filled} bytes of a response it says is {size} bytes long"
        );
        if filled == size {
            response.truncate(size);
            return Ok(response);
        }
    }
}

impl Tpm {
    pub(crate) fn open(path: &str) -> std::io::Result<Arc<Tpm>> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt as _;
            if std::fs::metadata(path)?.file_type().is_socket() {
                let socket = std::os::unix::net::UnixStream::connect(path)?;
                return Ok(Arc::new(Tpm {
                    path: path.to_string(),
                    channel: Mutex::new(Channel::Socket(socket)),
                }));
            }
        }
        let device = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        Ok(Arc::new(Tpm {
            path: path.to_string(),
            channel: Mutex::new(Channel::Device(device)),
        }))
    }

    fn execute<C: TpmFrame, R: TpmUnmarshalBody>(
        &self,
        name: &'static str,
        command: &C,
        sessions: &[TpmsAuthCommand],
    ) -> anyhow::Result<R> {
        let mut buffer = vec![0u8; MAXIMUM_FRAME_BYTES];
        let mut writer = TpmWriter::new(&mut buffer);
        let tag = if sessions.is_empty() {
            TpmSt::NoSessions
        } else {
            TpmSt::Sessions
        };
        tpm_marshal_command(command, tag, sessions, &mut writer)
            .with_context(|| format!("couldn't marshal {name}"))?;
        let length = writer.len();
        let mut attempt = 1;
        loop {
            let response = {
                let mut channel = self
                    .channel
                    .lock()
                    .map_err(|_| anyhow::anyhow!("an earlier command to the TPM panicked"))?;
                match &mut *channel {
                    Channel::Device(device) => exchange(device, &buffer[..length]),
                    #[cfg(unix)]
                    Channel::Socket(socket) => exchange(socket, &buffer[..length]),
                }
            }
            .with_context(|| format!("{name} failed"))?;
            let code = u32::from_be_bytes([response[6], response[7], response[8], response[9]]);
            if code == 0 {
                return TpmResponse::cast(&response)
                    .and_then(|view| view.unmarshal::<R>())
                    .with_context(|| format!("the TPM's response to {name} is malformed"));
            }
            let refusal = ResponseCode {
                command: name,
                code,
            };
            if attempt == MAXIMUM_ATTEMPTS || !refusal.asks_to_retry() {
                return Err(refusal.into());
            }
            tracing::debug!(
                tpm = self.path.as_str(),
                command = name,
                attempt,
                "the TPM asked for a command again"
            );
            std::thread::sleep(RETRY_PAUSE);
            attempt += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Chunked {
        written: Vec<u8>,
        response: Vec<u8>,
        chunk: usize,
    }

    impl Read for Chunked {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.chunk.min(self.response.len()).min(buffer.len());
            buffer[..count].copy_from_slice(&self.response[..count]);
            self.response.drain(..count);
            Ok(count)
        }
    }

    impl Write for Chunked {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn response_of(size: u32, total: usize) -> Vec<u8> {
        let mut response = vec![0x80, 0x01];
        response.extend_from_slice(&size.to_be_bytes());
        response.resize(total, 0);
        response
    }

    #[test]
    fn a_response_is_read_whole_however_it_arrives() {
        for chunk in [1, 3, 10, 4096] {
            let mut channel = Chunked {
                written: Vec::new(),
                response: response_of(14, 14),
                chunk,
            };
            assert_eq!(
                exchange(&mut channel, b"command").unwrap(),
                response_of(14, 14)
            );
            assert_eq!(channel.written, b"command");
        }
    }

    #[test]
    fn a_short_or_oversized_response_is_an_error() {
        for (size, total) in [(14, 12), (9, 10), (4097, 4096), (10, 12)] {
            let mut channel = Chunked {
                written: Vec::new(),
                response: response_of(size, total),
                chunk: 4096,
            };
            assert!(
                exchange(&mut channel, b"command").is_err(),
                "{size} {total}"
            );
        }
    }

    #[test]
    fn response_codes_name_the_command_and_the_code() {
        let missing = ResponseCode {
            command: "NV_ReadPublic",
            code: 0x18b,
        };
        assert!(missing.is(TpmRcBase::Handle));
        assert!(!missing.is(TpmRcBase::Integrity));
        assert!(!missing.asks_to_retry());
        for code in [0x922, 0x908, 0x90a] {
            assert!(
                ResponseCode {
                    command: "NV_Read",
                    code
                }
                .asks_to_retry(),
                "{code:#x}"
            );
        }
        assert!(
            missing
                .to_string()
                .starts_with("the TPM refused NV_ReadPublic with response code 0x18b")
        );
        let unknown = ResponseCode {
            command: "Sign",
            code: 0xffff_ffff,
        };
        assert!(!unknown.is(TpmRcBase::Handle));
        assert_eq!(
            unknown.to_string(),
            "the TPM refused Sign with response code 0xffffffff"
        );
    }
}
