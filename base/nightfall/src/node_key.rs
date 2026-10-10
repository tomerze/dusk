use std::prelude::rust_2024::*;

use std::sync::Arc;

use dusk_program::value::Value;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};
use rustls::SignatureScheme;
use rustls::pki_types::SubjectPublicKeyInfoDer;
use tpm2_protocol::data::TpmRh;

use crate::tpm::{self, KeyBlob, Tpm};

pub(crate) enum NodeKey {
    Software {
        pair: EcdsaKeyPair,
        random: SystemRandom,
        pkcs8: Vec<u8>,
    },
    Tpm {
        object: tpm::Object,
        private: Vec<u8>,
        point: Vec<u8>,
    },
}

impl NodeKey {
    pub(crate) fn generate() -> anyhow::Result<NodeKey> {
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                .map_err(|_| anyhow::anyhow!("the system random number generator failed"))?;
        NodeKey::from_pkcs8(pkcs8.as_ref())
            .map_err(|reason| anyhow::anyhow!("a freshly generated key is unusable: {reason}"))
    }

    pub(crate) fn generate_in(tpm: &Arc<Tpm>) -> anyhow::Result<NodeKey> {
        let storage_root = tpm.create_primary(TpmRh::Owner, tpm::storage_root_template())?;
        let blob = storage_root.create(tpm::node_key_template())?;
        NodeKey::load(&storage_root, blob)
    }

    pub(crate) fn generate_like(&self) -> anyhow::Result<NodeKey> {
        match self {
            NodeKey::Software { .. } => NodeKey::generate(),
            NodeKey::Tpm { object, .. } => NodeKey::generate_in(object.tpm()),
        }
    }

    fn load(storage_root: &tpm::Object, blob: KeyBlob) -> anyhow::Result<NodeKey> {
        let point = tpm::node_key_point(&blob.public)
            .map_err(|reason| anyhow::anyhow!("the TPM created an unusable node key: {reason}"))?;
        let object = storage_root.load(&blob)?;
        Ok(NodeKey::Tpm {
            object,
            private: blob.private,
            point,
        })
    }

    pub(crate) fn from_pkcs8(pkcs8: &[u8]) -> Result<NodeKey, String> {
        let random = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &random)
            .map_err(|error| format!("it is not a PKCS#8 ECDSA P-256 private key: {error}"))?;
        Ok(NodeKey::Software {
            pair,
            random,
            pkcs8: pkcs8.to_vec(),
        })
    }

    pub(crate) fn public_key_of(stored: &Value) -> Result<Vec<u8>, String> {
        match StoredKey::read(stored)? {
            StoredKey::Pkcs8(pkcs8) => Ok(NodeKey::from_pkcs8(pkcs8)?.public_key().to_vec()),
            StoredKey::Tpm { public, .. } => tpm::node_key_point(public),
        }
    }

    pub(crate) fn from_stored(stored: &Value, tpm: Option<&Arc<Tpm>>) -> Result<NodeKey, String> {
        let (private, public) = match StoredKey::read(stored)? {
            StoredKey::Pkcs8(pkcs8) => return NodeKey::from_pkcs8(pkcs8),
            StoredKey::Tpm { private, public } => (private, public),
        };
        let tpm =
            tpm.ok_or_else(|| String::from("it is kept in a TPM, and nightfall opened none"))?;
        tpm.create_primary(TpmRh::Owner, tpm::storage_root_template())
            .and_then(|storage_root| {
                NodeKey::load(
                    &storage_root,
                    KeyBlob {
                        private: private.to_vec(),
                        public: public.to_vec(),
                    },
                )
            })
            .map_err(|error| format!("the TPM at {} did not load it: {error:#}", tpm.path))
    }

    pub(crate) fn stored(&self) -> Value {
        match self {
            NodeKey::Software { pkcs8, .. } => Value::Bytes(pkcs8.clone()),
            NodeKey::Tpm {
                object, private, ..
            } => Value::List(vec![
                Value::Bytes(private.clone()),
                Value::Bytes(object.public.clone()),
            ]),
        }
    }

    pub(crate) fn tpm_object(&self) -> Option<&tpm::Object> {
        match self {
            NodeKey::Software { .. } => None,
            NodeKey::Tpm { object, .. } => Some(object),
        }
    }

    pub(crate) fn public_key(&self) -> &[u8] {
        match self {
            NodeKey::Software { pair, .. } => pair.public_key().as_ref(),
            NodeKey::Tpm { point, .. } => point,
        }
    }

    pub(crate) fn sign(&self, message: &[u8]) -> anyhow::Result<Vec<u8>> {
        match self {
            NodeKey::Software { pair, random, .. } => pair
                .sign(random, message)
                .map(|signature| signature.as_ref().to_vec())
                .map_err(|_| anyhow::anyhow!("ECDSA signing failed")),
            NodeKey::Tpm { object, .. } => {
                object.sign(ring::digest::digest(&ring::digest::SHA256, message).as_ref())
            }
        }
    }
}

enum StoredKey<'stored> {
    Pkcs8(&'stored [u8]),
    Tpm {
        private: &'stored [u8],
        public: &'stored [u8],
    },
}

impl<'stored> StoredKey<'stored> {
    fn read(stored: &'stored Value) -> Result<StoredKey<'stored>, String> {
        if let Value::Bytes(pkcs8) = stored {
            return Ok(StoredKey::Pkcs8(pkcs8));
        }
        if let Value::List(parts) = stored
            && let [Value::Bytes(private), Value::Bytes(public)] = parts.as_slice()
        {
            return Ok(StoredKey::Tpm { private, public });
        }
        Err(String::from(
            "it is neither a PKCS#8 key nor a TPM private and public area",
        ))
    }
}

impl core::fmt::Debug for NodeKey {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("NodeKey")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

pub(crate) struct CsrSigningKey<'key>(pub &'key NodeKey);

impl rcgen::PublicKeyData for CsrSigningKey<'_> {
    fn der_bytes(&self) -> &[u8] {
        self.0.public_key()
    }

    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}

impl rcgen::SigningKey for CsrSigningKey<'_> {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        self.0.sign(message).map_err(|error| {
            tracing::error!(error = %format_args!("{error:#}"), "the node key couldn't sign a certificate request");
            rcgen::Error::RemoteKeyError
        })
    }
}

#[derive(Debug)]
pub(crate) struct TlsSigningKey {
    key: Arc<NodeKey>,
    subject_public_key_info: Vec<u8>,
}

impl TlsSigningKey {
    pub(crate) fn new(key: Arc<NodeKey>) -> Self {
        let subject_public_key_info =
            rcgen::PublicKeyData::subject_public_key_info(&CsrSigningKey(key.as_ref()));
        TlsSigningKey {
            key,
            subject_public_key_info,
        }
    }
}

impl rustls::sign::SigningKey for TlsSigningKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn rustls::sign::Signer>> {
        offered
            .contains(&SignatureScheme::ECDSA_NISTP256_SHA256)
            .then(|| Box::new(TlsSigner(self.key.clone())) as Box<dyn rustls::sign::Signer>)
    }

    fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'_>> {
        Some(SubjectPublicKeyInfoDer::from(
            self.subject_public_key_info.as_slice(),
        ))
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ECDSA
    }
}

#[derive(Debug)]
struct TlsSigner(Arc<NodeKey>);

impl rustls::sign::Signer for TlsSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        self.0.sign(message).map_err(|error| {
            rustls::Error::General(format!("the node key couldn't sign: {error:#}"))
        })
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::ECDSA_NISTP256_SHA256
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
    use rustls::sign::SigningKey as _;
    use x509_parser::prelude::FromDer as _;

    #[test]
    fn a_generated_key_reads_back_from_what_it_stores() {
        let key = NodeKey::generate().unwrap();
        let stored = key.stored();
        assert!(matches!(&stored, Value::Bytes(_)));
        assert_eq!(NodeKey::public_key_of(&stored).unwrap(), key.public_key());
        let read_back = NodeKey::from_stored(&stored, None).unwrap();
        assert_eq!(read_back.public_key(), key.public_key());
        assert!(read_back.tpm_object().is_none());
        let signature = read_back.sign(b"dusk").unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, key.public_key())
            .verify(b"dusk", &signature)
            .unwrap();
    }

    #[test]
    fn anything_but_a_pkcs8_p256_key_is_refused() {
        let ed25519 =
            ring::signature::Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        for unusable in [
            &b""[..],
            b"garbage",
            &[0x30, 0x03, 0x02, 0x01, 0x01],
            ed25519.as_ref(),
        ] {
            assert!(NodeKey::from_pkcs8(unusable).is_err(), "{unusable:?}");
        }
    }

    #[test]
    fn a_stored_value_is_a_pkcs8_key_or_a_tpm_private_and_public_area() {
        let mut node_key = tpm::node_key_template();
        node_key.unique =
            tpm2_protocol::data::TpmuPublicId::Ecc(tpm2_protocol::data::TpmsEccPoint {
                x: tpm2_protocol::data::Tpm2bEccParameter::try_from(&[0x11u8; 32][..]).unwrap(),
                y: tpm2_protocol::data::Tpm2bEccParameter::try_from(&[0x22u8; 32][..]).unwrap(),
            });
        let public = tpm::marshal(&node_key).unwrap();
        let in_a_tpm = Value::List(vec![Value::Bytes(vec![0xaa; 126]), Value::Bytes(public)]);
        let point = NodeKey::public_key_of(&in_a_tpm).unwrap();
        assert_eq!(point.len(), 65);
        assert_eq!(&point[1..33], &[0x11; 32]);
        assert_eq!(
            NodeKey::from_stored(&in_a_tpm, None).unwrap_err(),
            "it is kept in a TPM, and nightfall opened none"
        );
        for unusable in [
            Value::String(String::from("not a key")),
            Value::List(vec![Value::Bytes(vec![0xaa; 126])]),
            Value::List(vec![Value::Bytes(vec![0xaa]), Value::String(String::new())]),
            Value::List(vec![
                Value::Bytes(vec![0xaa]),
                Value::Bytes(b"not a public area".to_vec()),
            ]),
            Value::Bytes(b"garbage".to_vec()),
        ] {
            assert!(NodeKey::public_key_of(&unusable).is_err(), "{unusable:?}");
        }
    }

    #[test]
    fn the_tls_adapter_offers_only_ecdsa_p256_sha256() {
        let key = NodeKey::generate().unwrap();
        let key = Arc::new(key);
        let adapter = TlsSigningKey::new(key.clone());
        assert!(
            adapter
                .choose_scheme(&[SignatureScheme::ED25519, SignatureScheme::RSA_PSS_SHA256])
                .is_none()
        );
        let signer = adapter
            .choose_scheme(&[
                SignatureScheme::RSA_PSS_SHA256,
                SignatureScheme::ECDSA_NISTP256_SHA256,
            ])
            .unwrap();
        assert_eq!(signer.scheme(), SignatureScheme::ECDSA_NISTP256_SHA256);
        let signature = signer.sign(b"handshake").unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, key.public_key())
            .verify(b"handshake", &signature)
            .unwrap();
        let public_key_info = adapter.public_key().unwrap();
        let (_, parsed) =
            x509_parser::x509::SubjectPublicKeyInfo::from_der(&public_key_info).unwrap();
        assert_eq!(parsed.subject_public_key.data.as_ref(), key.public_key());
    }

    #[test]
    fn debug_never_shows_the_private_key() {
        let key = NodeKey::generate().unwrap();
        assert_eq!(
            format!("{key:?}"),
            format!("NodeKey {{ public_key: {:?}, .. }}", key.public_key())
        );
    }
}
