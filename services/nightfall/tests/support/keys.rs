use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};

pub struct EcdsaKey {
    pkcs8: Vec<u8>,
    key: EcdsaKeyPair,
    pub kid: String,
}

impl EcdsaKey {
    pub fn new(kid: &str) -> EcdsaKey {
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                .unwrap();
        EcdsaKey {
            pkcs8: pkcs8.as_ref().to_vec(),
            key,
            kid: kid.to_string(),
        }
    }

    pub fn public_jwk(&self) -> Value {
        let point = self.key.public_key().as_ref();
        json!({
            "kty": "EC",
            "crv": "P-256",
            "kid": self.kid,
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
        })
    }

    pub fn private_jwk(&self) -> Value {
        let mut jwk = self.public_jwk();
        jwk["d"] = Value::String(URL_SAFE_NO_PAD.encode(&self.pkcs8[36..68]));
        jwk
    }
}

pub struct LedgerKey {
    pub pkcs8: Vec<u8>,
}

impl LedgerKey {
    pub fn new() -> LedgerKey {
        LedgerKey {
            pkcs8: Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .unwrap()
                .as_ref()
                .to_vec(),
        }
    }

    pub fn pem(&self) -> String {
        let encoded = STANDARD.encode(&self.pkcs8);
        let mut pem = String::from("-----BEGIN PRIVATE KEY-----\n");
        for line in encoded.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap());
            pem.push('\n');
        }
        pem.push_str("-----END PRIVATE KEY-----\n");
        pem
    }

    pub fn public_jwks(&self) -> String {
        nightfall_ledger::signing::CheckpointSigner::from_pkcs8_der(&self.pkcs8)
            .unwrap()
            .public_jwks()
            .to_string()
    }
}

pub fn hex_key(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}
