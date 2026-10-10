use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use rustls_pki_types::PrivateKeyDer;
use rustls_pki_types::pem::PemObject;
use serde_json::{Value, json};

use crate::canonical::canonical_bytes;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("{0}")]
    Invalid(String),
}

pub struct CheckpointSigner {
    key_pair: Ed25519KeyPair,
    key_id: String,
}

impl CheckpointSigner {
    pub fn load(path: &Path) -> Result<CheckpointSigner, KeyError> {
        let pem = std::fs::read(path).map_err(|source| KeyError::Read {
            path: path.display().to_string(),
            source,
        })?;
        CheckpointSigner::from_pkcs8_pem(&pem)
            .map_err(|error| KeyError::Invalid(format!("{}: {error}", path.display())))
    }

    pub fn from_pkcs8_pem(pem: &[u8]) -> Result<CheckpointSigner, KeyError> {
        let key = PrivateKeyDer::from_pem_slice(pem)
            .map_err(|error| KeyError::Invalid(format!("not a PEM private key: {error}")))?;
        let PrivateKeyDer::Pkcs8(pkcs8) = key else {
            return Err(KeyError::Invalid(String::from(
                "the ledger signing key must be a PKCS#8 Ed25519 key",
            )));
        };
        CheckpointSigner::from_pkcs8_der(pkcs8.secret_pkcs8_der())
    }

    pub fn from_pkcs8_der(der: &[u8]) -> Result<CheckpointSigner, KeyError> {
        let key_pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(der)
            .map_err(|error| KeyError::Invalid(format!("not a PKCS#8 Ed25519 key: {error}")))?;
        let key_id = thumbprint(key_pair.public_key().as_ref());
        Ok(CheckpointSigner { key_pair, key_id })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn sign(&self, message: &[u8]) -> String {
        STANDARD.encode(self.key_pair.sign(message).as_ref())
    }

    pub fn public_jwks(&self) -> Value {
        json!({
            "keys": [{
                "kty": "OKP",
                "crv": "Ed25519",
                "x": URL_SAFE_NO_PAD.encode(self.key_pair.public_key().as_ref()),
                "kid": self.key_id,
                "use": "sig",
                "alg": "EdDSA",
            }]
        })
    }
}

pub fn thumbprint(public_key: &[u8]) -> String {
    let members =
        json!({ "crv": "Ed25519", "kty": "OKP", "x": URL_SAFE_NO_PAD.encode(public_key) });
    let canonical = canonical_bytes(&members).expect("a JWK of strings always canonicalizes");
    URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, &canonical).as_ref())
}

pub fn checkpoint_message(instance: &str, partition: u32, sequence: u64, hash: &str) -> Vec<u8> {
    let members =
        json!({ "instance": instance, "partition": partition, "sequence": sequence, "hash": hash });
    canonical_bytes(&members).expect("sequence numbers stay below 2^53")
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureProblem {
    #[error("no configured public key has key id {0}")]
    UnknownKey(String),
    #[error("the signature is not base64 of 64 bytes")]
    Malformed,
    #[error("the signature does not verify")]
    Invalid,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyingKeys {
    keys: HashMap<String, Vec<u8>>,
}

impl VerifyingKeys {
    pub fn load(path: &Path) -> Result<VerifyingKeys, KeyError> {
        let text = std::fs::read_to_string(path).map_err(|source| KeyError::Read {
            path: path.display().to_string(),
            source,
        })?;
        VerifyingKeys::from_jwks(&text)
            .map_err(|error| KeyError::Invalid(format!("{}: {error}", path.display())))
    }

    pub fn from_jwks(text: &str) -> Result<VerifyingKeys, KeyError> {
        let document: Value = serde_json::from_str(text)
            .map_err(|error| KeyError::Invalid(format!("not JSON: {error}")))?;
        let Some(keys) = document.get("keys").and_then(Value::as_array) else {
            return Err(KeyError::Invalid(String::from("a JWKS needs a keys array")));
        };
        let mut verifying_keys = VerifyingKeys::default();
        for (position, key) in keys.iter().enumerate() {
            let member = |name: &str| key.get(name).and_then(Value::as_str);
            if member("kty") != Some("OKP") || member("crv") != Some("Ed25519") {
                return Err(KeyError::Invalid(format!(
                    "key {position} is not an OKP Ed25519 key"
                )));
            }
            let public_key = member("x")
                .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
                .filter(|decoded| decoded.len() == 32)
                .ok_or_else(|| KeyError::Invalid(format!("key {position} has no 32-byte x")))?;
            let key_id = thumbprint(&public_key);
            if let Some(declared) = member("kid")
                && declared != key_id
            {
                return Err(KeyError::Invalid(format!(
                    "key {position} declares kid {declared} but its RFC 7638 thumbprint is {key_id}"
                )));
            }
            verifying_keys.keys.insert(key_id, public_key);
        }
        Ok(verifying_keys)
    }

    pub fn verify(
        &self,
        key_id: &str,
        message: &[u8],
        signature: &str,
    ) -> Result<(), SignatureProblem> {
        let public_key = self
            .keys
            .get(key_id)
            .ok_or_else(|| SignatureProblem::UnknownKey(String::from(key_id)))?;
        let signature = STANDARD
            .decode(signature)
            .ok()
            .filter(|decoded| decoded.len() == 64)
            .ok_or(SignatureProblem::Malformed)?;
        UnparsedPublicKey::new(&ED25519, public_key)
            .verify(message, &signature)
            .map_err(|_| SignatureProblem::Invalid)
    }
}

#[cfg(test)]
pub mod fixtures {
    use super::*;

    pub fn signer(seed: u8) -> CheckpointSigner {
        let mut pkcs8 = vec![
            0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
            0x04, 0x20,
        ];
        pkcs8.extend_from_slice(&[seed; 32]);
        CheckpointSigner::from_pkcs8_der(&pkcs8).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::signer;
    use super::*;

    #[test]
    fn signs_and_verifies_through_a_jwks() {
        let signer = signer(7);
        let keys = VerifyingKeys::from_jwks(&signer.public_jwks().to_string()).unwrap();
        let message = checkpoint_message("nightfall-0", 0, 41, &"a".repeat(64));
        let signature = signer.sign(&message);
        assert_eq!(signature.len(), 88);
        keys.verify(signer.key_id(), &message, &signature).unwrap();
        let altered = checkpoint_message("nightfall-0", 0, 42, &"a".repeat(64));
        assert_eq!(
            keys.verify(signer.key_id(), &altered, &signature),
            Err(SignatureProblem::Invalid)
        );
        assert_eq!(
            keys.verify("other", &message, &signature),
            Err(SignatureProblem::UnknownKey(String::from("other")))
        );
        assert_eq!(
            keys.verify(signer.key_id(), &message, "AAAA"),
            Err(SignatureProblem::Malformed)
        );
    }

    #[test]
    fn computes_the_rfc_8037_thumbprint() {
        let public_key = URL_SAFE_NO_PAD
            .decode("11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo")
            .unwrap();
        assert_eq!(
            thumbprint(&public_key),
            "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k"
        );
    }

    #[test]
    fn loads_an_openssl_pkcs8_pem() {
        let pem = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEINTuctv5E1hK1bbY8fdp+K06/nwoy/HU++CXqI9EdVhC\n-----END PRIVATE KEY-----\n";
        let signer = CheckpointSigner::from_pkcs8_pem(pem).unwrap();
        assert_eq!(signer.key_id().len(), 43);
    }

    #[test]
    fn refuses_a_jwks_whose_kid_is_not_the_thumbprint() {
        let mut jwks = signer(1).public_jwks();
        jwks["keys"][0]["kid"] = Value::from("nightfall-ledger-2026-10");
        assert!(VerifyingKeys::from_jwks(&jwks.to_string()).is_err());
    }

    #[test]
    fn writes_the_signed_message_canonically() {
        assert_eq!(
            String::from_utf8(checkpoint_message("nightfall-0", 3, 41, "ab")).unwrap(),
            r#"{"hash":"ab","instance":"nightfall-0","partition":3,"sequence":41}"#
        );
    }
}
