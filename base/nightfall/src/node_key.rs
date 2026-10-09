use std::prelude::rust_2024::*;

use std::sync::Arc;

use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};
use rustls::SignatureScheme;
use rustls::pki_types::SubjectPublicKeyInfoDer;

pub(crate) struct NodeKey {
    pair: EcdsaKeyPair,
    random: SystemRandom,
}

impl NodeKey {
    pub(crate) fn generate() -> anyhow::Result<(NodeKey, Vec<u8>)> {
        let random = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &random)
            .map_err(|_| anyhow::anyhow!("the system random number generator failed"))?;
        let key = NodeKey::from_pkcs8(pkcs8.as_ref())
            .map_err(|reason| anyhow::anyhow!("a freshly generated key is unusable: {reason}"))?;
        Ok((key, pkcs8.as_ref().to_vec()))
    }

    pub(crate) fn from_pkcs8(pkcs8: &[u8]) -> Result<NodeKey, String> {
        let random = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &random)
            .map_err(|error| format!("it is not a PKCS#8 ECDSA P-256 private key: {error}"))?;
        Ok(NodeKey { pair, random })
    }

    pub(crate) fn public_key(&self) -> &[u8] {
        self.pair.public_key().as_ref()
    }

    pub(crate) fn sign(&self, message: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.pair
            .sign(&self.random, message)
            .map(|signature| signature.as_ref().to_vec())
            .map_err(|_| anyhow::anyhow!("ECDSA signing failed"))
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
    fn a_generated_key_reads_back_from_its_pkcs8() {
        let (key, pkcs8) = NodeKey::generate().unwrap();
        let read_back = NodeKey::from_pkcs8(&pkcs8).unwrap();
        assert_eq!(read_back.public_key(), key.public_key());
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
    fn the_tls_adapter_offers_only_ecdsa_p256_sha256() {
        let (key, _) = NodeKey::generate().unwrap();
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
        let (key, _) = NodeKey::generate().unwrap();
        assert_eq!(
            format!("{key:?}"),
            format!("NodeKey {{ public_key: {:?}, .. }}", key.public_key())
        );
    }
}
