use std::prelude::rust_2024::*;

use std::io::{ErrorKind, Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use tpm2_protocol::basic::{TpmHandle, TpmUint16};
use tpm2_protocol::data::{
    Tpm2bDigest, Tpm2bNonce, Tpm2bPrivate, Tpm2bPublic, TpmAlgId, TpmEccCurve, TpmRc, TpmRcBase,
    TpmRh, TpmSt, TpmaObject, TpmaSession, TpmsAuthCommand, TpmsEccParms, TpmsEccPoint,
    TpmsSchemeHash, TpmtEccScheme, TpmtKdfScheme, TpmtPublic, TpmtSigScheme, TpmtSymDef,
    TpmtTkHashcheck, TpmuAsymScheme, TpmuPublicId, TpmuPublicParms, TpmuSignature, TpmuSymKeyBits,
    TpmuSymMode,
};
use tpm2_protocol::frame::{
    TpmCreateCommand, TpmCreatePrimaryCommand, TpmCreatePrimaryResponse, TpmCreateResponse,
    TpmFlushContextCommand, TpmFlushContextResponse, TpmFrame, TpmLoadCommand, TpmLoadResponse,
    TpmResponse, TpmSignCommand, TpmSignResponse, TpmUnmarshalBody, tpm_marshal_command,
};
use tpm2_protocol::{TpmError, TpmMarshal, TpmUnmarshal, TpmWriter};

const MAXIMUM_FRAME_BYTES: usize = 4096;
const HEADER_BYTES: usize = 10;
const P256_COORDINATE_BYTES: usize = 32;
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

pub(crate) struct Object {
    tpm: Arc<Tpm>,
    handle: u32,
    pub(crate) public: Vec<u8>,
}

impl Drop for Object {
    fn drop(&mut self) {
        if let Err(error) = self.tpm.flush(self.handle) {
            tracing::warn!(
                tpm = self.tpm.path.as_str(),
                handle = self.handle,
                error = %format_args!("{error:#}"),
                "couldn't flush an object from the TPM"
            );
        }
    }
}

pub(crate) struct KeyBlob {
    pub(crate) private: Vec<u8>,
    pub(crate) public: Vec<u8>,
}

fn password() -> TpmsAuthCommand {
    session(TpmRh::Pw.value())
}

fn session(handle: u32) -> TpmsAuthCommand {
    TpmsAuthCommand {
        session_handle: TpmHandle::new(handle),
        nonce: Tpm2bNonce::default(),
        session_attributes: TpmaSession::default(),
        hmac: Default::default(),
    }
}

pub(crate) fn marshal(value: &impl TpmMarshal) -> Result<Vec<u8>, TpmError> {
    let mut buffer = vec![0u8; MAXIMUM_FRAME_BYTES];
    let mut writer = TpmWriter::new(&mut buffer);
    value.marshal(&mut writer)?;
    let length = writer.len();
    buffer.truncate(length);
    Ok(buffer)
}

fn parse_public(public: &[u8]) -> Result<TpmtPublic, String> {
    let (parsed, remainder) = TpmtPublic::unmarshal(public)
        .map_err(|error| format!("it is not a TPM public area: {error}"))?;
    if !remainder.is_empty() {
        return Err(String::from("trailing bytes after the TPM public area"));
    }
    Ok(parsed)
}

fn sha256_name(public: &[u8]) -> Vec<u8> {
    let mut name = TpmAlgId::Sha256.value().to_be_bytes().to_vec();
    name.extend_from_slice(ring::digest::digest(&ring::digest::SHA256, public).as_ref());
    name
}

fn aes_128_cfb() -> TpmtSymDef {
    TpmtSymDef {
        algorithm: TpmAlgId::Aes,
        key_bits: TpmuSymKeyBits::Aes(TpmUint16::new(128)),
        mode: TpmuSymMode::Aes(TpmAlgId::Cfb),
    }
}

fn no_symmetric() -> TpmtSymDef {
    TpmtSymDef {
        algorithm: TpmAlgId::Null,
        key_bits: TpmuSymKeyBits::Null,
        mode: TpmuSymMode::Null,
    }
}

pub(crate) fn storage_root_template() -> TpmtPublic {
    TpmtPublic {
        object_type: TpmAlgId::Ecc,
        name_alg: TpmAlgId::Sha256,
        object_attributes: TpmaObject::FIXED_TPM
            | TpmaObject::FIXED_PARENT
            | TpmaObject::SENSITIVE_DATA_ORIGIN
            | TpmaObject::USER_WITH_AUTH
            | TpmaObject::NO_DA
            | TpmaObject::RESTRICTED
            | TpmaObject::DECRYPT,
        auth_policy: Tpm2bDigest::default(),
        parameters: TpmuPublicParms::Ecc(TpmsEccParms {
            symmetric: aes_128_cfb(),
            scheme: TpmtEccScheme::default(),
            curve_id: TpmEccCurve::NistP256,
            kdf: TpmtKdfScheme::default(),
        }),
        unique: TpmuPublicId::Ecc(TpmsEccPoint::default()),
    }
}

pub(crate) fn node_key_template() -> TpmtPublic {
    TpmtPublic {
        object_type: TpmAlgId::Ecc,
        name_alg: TpmAlgId::Sha256,
        object_attributes: TpmaObject::FIXED_TPM
            | TpmaObject::FIXED_PARENT
            | TpmaObject::SENSITIVE_DATA_ORIGIN
            | TpmaObject::USER_WITH_AUTH
            | TpmaObject::NO_DA
            | TpmaObject::SIGN_ENCRYPT,
        auth_policy: Tpm2bDigest::default(),
        parameters: TpmuPublicParms::Ecc(TpmsEccParms {
            symmetric: no_symmetric(),
            scheme: TpmtEccScheme {
                scheme: TpmAlgId::Ecdsa,
                details: TpmuAsymScheme::Hash(TpmsSchemeHash {
                    hash_alg: TpmAlgId::Sha256,
                }),
            },
            curve_id: TpmEccCurve::NistP256,
            kdf: TpmtKdfScheme::default(),
        }),
        unique: TpmuPublicId::Ecc(TpmsEccPoint::default()),
    }
}

pub(crate) fn node_key_point(public: &[u8]) -> Result<Vec<u8>, String> {
    let parsed = parse_public(public)?;
    let TpmuPublicId::Ecc(point) = &parsed.unique else {
        return Err(String::from("it is not an ECC key"));
    };
    let mut expected = node_key_template();
    expected.unique = parsed.unique.clone();
    if marshal(&expected).map_err(|error| error.to_string())? != public {
        return Err(String::from("it does not follow the node key template"));
    }
    let mut uncompressed = vec![0x04];
    for coordinate in [&point.x[..], &point.y[..]] {
        if coordinate.len() > P256_COORDINATE_BYTES {
            return Err(String::from("its point is not on P-256"));
        }
        uncompressed.resize(
            uncompressed.len() + P256_COORDINATE_BYTES - coordinate.len(),
            0,
        );
        uncompressed.extend_from_slice(coordinate);
    }
    Ok(uncompressed)
}

pub(crate) fn der_signature(r: &[u8], s: &[u8]) -> Vec<u8> {
    let integer = |bytes: &[u8]| {
        let start = bytes
            .iter()
            .position(|byte| *byte != 0)
            .unwrap_or(bytes.len());
        let magnitude = &bytes[start..];
        let pad = magnitude.first().is_none_or(|byte| byte & 0x80 != 0);
        let mut encoded = vec![0x02, (magnitude.len() + usize::from(pad)) as u8];
        if pad {
            encoded.push(0);
        }
        encoded.extend_from_slice(magnitude);
        encoded
    };
    let mut body = integer(r);
    body.extend_from_slice(&integer(s));
    let mut encoded = vec![0x30, body.len() as u8];
    encoded.extend_from_slice(&body);
    encoded
}

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

    fn flush(&self, handle: u32) -> anyhow::Result<()> {
        let command = TpmFlushContextCommand {
            handles: [],
            flush_handle: TpmHandle::new(handle),
        };
        self.execute::<_, TpmFlushContextResponse>("FlushContext", &command, &[])
            .map(|_| ())
    }

    pub(crate) fn create_primary(
        self: &Arc<Self>,
        hierarchy: TpmRh,
        template: TpmtPublic,
    ) -> anyhow::Result<Object> {
        let command = TpmCreatePrimaryCommand {
            handles: [TpmHandle::new(hierarchy.value())],
            in_public: Tpm2bPublic { inner: template },
            ..Default::default()
        };
        let response: TpmCreatePrimaryResponse =
            self.execute("CreatePrimary", &command, &[password()])?;
        let object = Object {
            tpm: self.clone(),
            handle: response.handles[0].value(),
            public: marshal(&response.out_public.inner)?,
        };
        anyhow::ensure!(
            response.name[..] == sha256_name(&object.public)[..],
            "the TPM named the primary key it created after another public area"
        );
        Ok(object)
    }
}

impl Object {
    pub(crate) fn tpm(&self) -> &Arc<Tpm> {
        &self.tpm
    }

    pub(crate) fn create(&self, template: TpmtPublic) -> anyhow::Result<KeyBlob> {
        let command = TpmCreateCommand {
            handles: [TpmHandle::new(self.handle)],
            in_sensitive: Default::default(),
            in_public: Tpm2bPublic { inner: template },
            outside_info: Default::default(),
            creation_pcr: Default::default(),
        };
        let response: TpmCreateResponse = self.tpm.execute("Create", &command, &[password()])?;
        Ok(KeyBlob {
            private: response.out_private.to_vec(),
            public: marshal(&response.out_public.inner)?,
        })
    }

    pub(crate) fn load(&self, blob: &KeyBlob) -> anyhow::Result<Object> {
        let public = parse_public(&blob.public).map_err(|reason| anyhow::anyhow!("{reason}"))?;
        let command = TpmLoadCommand {
            handles: [TpmHandle::new(self.handle)],
            in_private: Tpm2bPrivate::try_from(blob.private.as_slice())?,
            in_public: Tpm2bPublic { inner: public },
        };
        let response: TpmLoadResponse = self.tpm.execute("Load", &command, &[password()])?;
        Ok(Object {
            tpm: self.tpm.clone(),
            handle: response.handles[0].value(),
            public: blob.public.clone(),
        })
    }

    pub(crate) fn sign(&self, digest: &[u8]) -> anyhow::Result<Vec<u8>> {
        let command = TpmSignCommand {
            handles: [TpmHandle::new(self.handle)],
            digest: Tpm2bDigest::try_from(digest)?,
            in_scheme: TpmtSigScheme::default(),
            validation: TpmtTkHashcheck {
                tag: TpmSt::HashCheck,
                hierarchy: TpmRh::Null,
                digest: Tpm2bDigest::default(),
            },
        };
        let response: TpmSignResponse = self.tpm.execute("Sign", &command, &[password()])?;
        let TpmuSignature::Ecdsa(signature) = response.signature.signature else {
            anyhow::bail!("the TPM answered Sign with a signature that is not ECDSA");
        };
        anyhow::ensure!(
            signature.signature_r.len() <= P256_COORDINATE_BYTES
                && signature.signature_s.len() <= P256_COORDINATE_BYTES,
            "the TPM answered Sign with an ECDSA signature that is not P-256"
        );
        Ok(der_signature(
            &signature.signature_r,
            &signature.signature_s,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpm2_protocol::data::Tpm2bEccParameter;

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn the_storage_root_template_is_an_ecc_p256_storage_key() {
        assert_eq!(
            marshal(&storage_root_template()).unwrap(),
            hex("0023 000b 00030472 0000 0006 0080 0043 0010 0003 0010 0000 0000")
        );
    }

    #[test]
    fn the_node_key_template_is_a_non_exportable_p256_ecdsa_signing_key() {
        assert_eq!(
            marshal(&node_key_template()).unwrap(),
            hex("0023 000b 00040472 0000 0010 0018 000b 0003 0010 0000 0000")
        );
    }

    fn node_key_public(x: &[u8], y: &[u8]) -> Vec<u8> {
        let mut public = node_key_template();
        public.unique = TpmuPublicId::Ecc(TpmsEccPoint {
            x: Tpm2bEccParameter::try_from(x).unwrap(),
            y: Tpm2bEccParameter::try_from(y).unwrap(),
        });
        marshal(&public).unwrap()
    }

    #[test]
    fn a_node_key_public_area_yields_its_uncompressed_point() {
        let x = [0x11u8; 32];
        let y = [0x22u8; 32];
        let point = node_key_point(&node_key_public(&x, &y)).unwrap();
        assert_eq!(point.len(), 65);
        assert_eq!(point[0], 0x04);
        assert_eq!(&point[1..33], &x);
        assert_eq!(&point[33..], &y);
        let short = node_key_point(&node_key_public(&[0x33; 31], &y)).unwrap();
        assert_eq!(&short[1..3], &[0x00, 0x33]);
    }

    #[test]
    fn a_public_area_that_is_not_a_node_key_is_refused() {
        let mut restricted = node_key_template();
        restricted.object_attributes |= TpmaObject::RESTRICTED;
        let mut trailing = node_key_public(&[0x11; 32], &[0x22; 32]);
        trailing.push(0);
        for public in [
            marshal(&restricted).unwrap(),
            marshal(&storage_root_template()).unwrap(),
            node_key_public(&[0x11; 33], &[0x22; 32]),
            trailing,
            b"not a public area".to_vec(),
        ] {
            assert!(node_key_point(&public).is_err(), "{public:02x?}");
        }
    }

    #[test]
    fn signatures_become_der_integers_that_stay_positive() {
        assert_eq!(
            der_signature(&[0x01; 32], &[0x7f; 32])[..6],
            [0x30, 0x44, 0x02, 0x20, 0x01, 0x01]
        );
        let high = der_signature(&[0x80; 32], &[0x00, 0x00, 0x05]);
        assert_eq!(high[..5], [0x30, 0x26, 0x02, 0x21, 0x00]);
        assert_eq!(high[high.len() - 3..], [0x02, 0x01, 0x05]);
        assert_eq!(
            der_signature(&[0x00; 32], &[0x01]),
            [0x30, 0x06, 0x02, 0x01, 0x00, 0x02, 0x01, 0x01]
        );
    }

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

    #[test]
    fn names_are_sha256_of_the_public_area() {
        let name = sha256_name(b"public");
        assert_eq!(name[..2], [0x00, 0x0b]);
        assert_eq!(
            name[2..],
            *ring::digest::digest(&ring::digest::SHA256, b"public").as_ref()
        );
    }
}
