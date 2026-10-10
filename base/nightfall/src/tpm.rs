use std::prelude::rust_2024::*;

use std::io::{ErrorKind, Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use ring::rand::SecureRandom as _;
use tpm2_protocol::basic::{TpmHandle, TpmInt32, TpmUint16, TpmUint32};
use tpm2_protocol::data::{
    Tpm2bDigest, Tpm2bEncryptedSecret, Tpm2bIdObject, Tpm2bNonce, Tpm2bPrivate, Tpm2bPublic,
    Tpm2bPublicKeyRsa, TpmAlgId, TpmEccCurve, TpmRc, TpmRcBase, TpmRh, TpmSe, TpmSt, TpmaObject,
    TpmaSession, TpmsAuthCommand, TpmsEccParms, TpmsEccPoint, TpmsRsaParms, TpmsSchemeHash,
    TpmtEccScheme, TpmtKdfScheme, TpmtPublic, TpmtRsaScheme, TpmtSigScheme, TpmtSymDef,
    TpmtTkHashcheck, TpmuAsymScheme, TpmuPublicId, TpmuPublicParms, TpmuSignature, TpmuSymKeyBits,
    TpmuSymMode,
};
use tpm2_protocol::frame::{
    TpmActivateCredentialCommand, TpmActivateCredentialResponse, TpmCreateCommand,
    TpmCreatePrimaryCommand, TpmCreatePrimaryResponse, TpmCreateResponse, TpmFlushContextCommand,
    TpmFlushContextResponse, TpmFrame, TpmLoadCommand, TpmLoadResponse, TpmNvReadCommand,
    TpmNvReadPublicCommand, TpmNvReadPublicResponse, TpmNvReadResponse, TpmPolicySecretCommand,
    TpmPolicySecretResponse, TpmResponse, TpmSignCommand, TpmSignResponse,
    TpmStartAuthSessionCommand, TpmStartAuthSessionResponse, TpmUnmarshalBody, tpm_marshal_command,
};
use tpm2_protocol::{TpmError, TpmMarshal, TpmUnmarshal, TpmWriter};

pub(crate) const ENDORSEMENT_CERTIFICATE_INDEX: u32 = 0x01C0_0002;
pub(crate) const FIRST_CHAIN_INDEX: u32 = 0x01C0_0100;
pub(crate) const LAST_CHAIN_INDEX: u32 = 0x01C0_01FF;
const NV_READ_CHUNK_BYTES: usize = 768;
const MAXIMUM_FRAME_BYTES: usize = 4096;
const HEADER_BYTES: usize = 10;
const P256_COORDINATE_BYTES: usize = 32;
const MAXIMUM_ATTEMPTS: u32 = 10;
const RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(20);
const ENDORSEMENT_POLICY: [u8; 32] = [
    0x83, 0x71, 0x97, 0x67, 0x44, 0x84, 0xB3, 0xF8, 0x1A, 0x90, 0xCC, 0x8D, 0x46, 0xA5, 0xD7, 0x24,
    0xFD, 0x52, 0xD7, 0x6E, 0x06, 0x52, 0x0B, 0x64, 0xF2, 0xA1, 0xDA, 0x1B, 0x33, 0x14, 0x69, 0xAA,
];

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

pub(crate) struct Endorsement {
    pub(crate) key: Object,
    pub(crate) certificate: Vec<u8>,
    pub(crate) chain: Vec<Vec<u8>>,
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

pub(crate) fn endorsement_key_template() -> Result<TpmtPublic, TpmError> {
    Ok(TpmtPublic {
        object_type: TpmAlgId::Rsa,
        name_alg: TpmAlgId::Sha256,
        object_attributes: TpmaObject::FIXED_TPM
            | TpmaObject::FIXED_PARENT
            | TpmaObject::SENSITIVE_DATA_ORIGIN
            | TpmaObject::ADMIN_WITH_POLICY
            | TpmaObject::RESTRICTED
            | TpmaObject::DECRYPT,
        auth_policy: Tpm2bDigest::try_from(&ENDORSEMENT_POLICY[..])?,
        parameters: TpmuPublicParms::Rsa(TpmsRsaParms {
            symmetric: aes_128_cfb(),
            scheme: TpmtRsaScheme::default(),
            key_bits: TpmUint16::new(2048),
            exponent: TpmUint32::new(0),
        }),
        unique: TpmuPublicId::Rsa(Tpm2bPublicKeyRsa::try_from(&[0u8; 256][..])?),
    })
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

pub(crate) fn certificates(mut contents: &[u8]) -> Vec<Vec<u8>> {
    let mut found = Vec::new();
    while let Some(length) = sequence_length(contents) {
        found.push(contents[..length].to_vec());
        contents = &contents[length..];
    }
    found
}

fn sequence_length(bytes: &[u8]) -> Option<usize> {
    if bytes.first() != Some(&0x30) {
        return None;
    }
    let first = usize::from(*bytes.get(1)?);
    let (header, body) = if first < 0x80 {
        (2, first)
    } else {
        let count = first - 0x80;
        if !(1..=3).contains(&count) {
            return None;
        }
        let length = bytes
            .get(2..2 + count)?
            .iter()
            .fold(0, |length, byte| (length << 8) | usize::from(*byte));
        (2 + count, length)
    };
    let total = header + body;
    (total <= bytes.len()).then_some(total)
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

    fn read_nv(&self, index: u32) -> anyhow::Result<Option<Vec<u8>>> {
        let command = TpmNvReadPublicCommand {
            handles: [TpmHandle::new(index)],
        };
        let public: TpmNvReadPublicResponse = match self.execute("NV_ReadPublic", &command, &[]) {
            Ok(public) => public,
            Err(error)
                if error
                    .downcast_ref::<ResponseCode>()
                    .is_some_and(|code| code.is(TpmRcBase::Handle)) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let size = usize::from(public.nv_public.inner.data_size.value());
        let mut contents = Vec::with_capacity(size);
        while contents.len() < size {
            let chunk = (size - contents.len()).min(NV_READ_CHUNK_BYTES);
            let command = TpmNvReadCommand {
                handles: [TpmHandle::new(index), TpmHandle::new(index)],
                size: TpmUint16::new(u16::try_from(chunk)?),
                offset: TpmUint16::new(u16::try_from(contents.len())?),
            };
            let response: TpmNvReadResponse = self.execute("NV_Read", &command, &[password()])?;
            anyhow::ensure!(
                response.data.len() == chunk,
                "the TPM read {} bytes of NV index {index:#x} when asked for {chunk}",
                response.data.len()
            );
            contents.extend_from_slice(&response.data);
        }
        Ok(Some(contents))
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

    pub(crate) fn endorsement(self: &Arc<Self>) -> anyhow::Result<Endorsement> {
        let certificate = self
            .read_nv(ENDORSEMENT_CERTIFICATE_INDEX)?
            .and_then(|contents| certificates(&contents).into_iter().next())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "the TPM holds no RSA endorsement key certificate at NV index {ENDORSEMENT_CERTIFICATE_INDEX:#x}"
                )
            })?;
        let mut chain = Vec::new();
        for index in FIRST_CHAIN_INDEX..=LAST_CHAIN_INDEX {
            let Some(contents) = self.read_nv(index)? else {
                break;
            };
            chain.extend(certificates(&contents));
        }
        let key = self.create_primary(TpmRh::Endorsement, endorsement_key_template()?)?;
        Ok(Endorsement {
            key,
            certificate,
            chain,
        })
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

    pub(crate) fn activate_credential(
        &self,
        endorsement_key: &Object,
        credential_blob: &[u8],
        encrypted_secret: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        let command = TpmActivateCredentialCommand {
            handles: [
                TpmHandle::new(self.handle),
                TpmHandle::new(endorsement_key.handle),
            ],
            credential_blob: Tpm2bIdObject::try_from(credential_blob)
                .context("the credential blob does not fit a TPM2B_ID_OBJECT")?,
            secret: Tpm2bEncryptedSecret::try_from(encrypted_secret)
                .context("the encrypted secret does not fit a TPM2B_ENCRYPTED_SECRET")?,
        };
        let mut nonce = [0u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("the system random number generator failed"))?;
        let start = TpmStartAuthSessionCommand {
            handles: [
                TpmHandle::new(TpmRh::Null.value()),
                TpmHandle::new(TpmRh::Null.value()),
            ],
            nonce_caller: Tpm2bNonce::try_from(&nonce[..])?,
            encrypted_salt: Tpm2bEncryptedSecret::default(),
            session_type: TpmSe::Policy,
            symmetric: no_symmetric(),
            auth_hash: TpmAlgId::Sha256,
        };
        let started: TpmStartAuthSessionResponse =
            self.tpm.execute("StartAuthSession", &start, &[])?;
        let policy_session = started.handles[0].value();
        let policy_secret = TpmPolicySecretCommand {
            handles: [
                TpmHandle::new(TpmRh::Endorsement.value()),
                TpmHandle::new(policy_session),
            ],
            nonce_tpm: Tpm2bNonce::default(),
            cp_hash_a: Tpm2bDigest::default(),
            policy_ref: Tpm2bNonce::default(),
            expiration: TpmInt32::new(0),
        };
        let activated = self
            .tpm
            .execute::<_, TpmPolicySecretResponse>("PolicySecret", &policy_secret, &[password()])
            .and_then(|_| {
                self.tpm.execute::<_, TpmActivateCredentialResponse>(
                    "ActivateCredential",
                    &command,
                    &[password(), session(policy_session)],
                )
            });
        match activated {
            Ok(response) => Ok(response.cert_info.to_vec()),
            Err(error) => {
                if let Err(flush_error) = self.tpm.flush(policy_session) {
                    tracing::warn!(
                        tpm = self.tpm.path.as_str(),
                        error = %format_args!("{flush_error:#}"),
                        "couldn't flush the policy session from the TPM"
                    );
                }
                Err(error)
            }
        }
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
    fn the_endorsement_key_template_is_tcg_template_l1() {
        let mut expected = hex("0001 000b 000300b2 0020
             837197674484b3f81a90cc8d46a5d724fd52d76e06520b64f2a1da1b331469aa
             0006 0080 0043 0010 0800 00000000 0100");
        expected.extend_from_slice(&[0u8; 256]);
        assert_eq!(
            marshal(&endorsement_key_template().unwrap()).unwrap(),
            expected
        );
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
        let mut endorsement = endorsement_key_template().unwrap();
        endorsement.unique =
            TpmuPublicId::Rsa(Tpm2bPublicKeyRsa::try_from(&[1u8; 256][..]).unwrap());
        let mut trailing = node_key_public(&[0x11; 32], &[0x22; 32]);
        trailing.push(0);
        for public in [
            marshal(&restricted).unwrap(),
            marshal(&storage_root_template()).unwrap(),
            marshal(&endorsement).unwrap(),
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

    #[test]
    fn concatenated_certificates_split_and_padding_ends_them() {
        let short = [0x30, 0x03, 0x02, 0x01, 0x07];
        let mut long = vec![0x30, 0x82, 0x01, 0x00];
        long.extend_from_slice(&[0x04; 0x100]);
        let mut contents = short.to_vec();
        contents.extend_from_slice(&long);
        contents.extend_from_slice(&[0xff; 16]);
        assert_eq!(certificates(&contents), vec![short.to_vec(), long.clone()]);
        assert!(certificates(&[]).is_empty());
        assert!(certificates(&[0x30, 0x05, 0x01]).is_empty());
        assert!(certificates(&[0x30, 0x84, 0, 0, 0, 1, 0]).is_empty());
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
