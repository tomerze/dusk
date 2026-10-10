use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING, ED25519, EcdsaKeyPair,
    UnparsedPublicKey,
};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JwtError {
    #[error("the token is not a compact JWS")]
    Malformed,
    #[error("the token header is not valid: {0}")]
    Header(String),
    #[error("the token is signed with {0}, which is not accepted")]
    Algorithm(String),
    #[error("no configured key has key id {0}")]
    UnknownKey(String),
    #[error("the token signature does not verify")]
    Signature,
    #[error("the token claims are not a JSON object")]
    Claims,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct KeyError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
enum PublicKey {
    P256(Vec<u8>),
    Ed25519(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JwksKey {
    public_key: PublicKey,
    max_installations: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct Jwks {
    keys: HashMap<String, JwksKey>,
}

fn member<'a>(key: &'a Value, name: &str) -> Option<&'a str> {
    key.get(name).and_then(Value::as_str)
}

fn decode_coordinate(key: &Value, name: &str, length: usize) -> Result<Vec<u8>, KeyError> {
    member(key, name)
        .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
        .filter(|decoded| decoded.len() == length)
        .ok_or_else(|| KeyError(format!("the key has no {length}-byte {name}")))
}

fn sha256_base64url(text: &str) -> String {
    URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, text.as_bytes()))
}

fn public_key_of(key: &Value) -> Result<(PublicKey, String), KeyError> {
    match (member(key, "kty"), member(key, "crv")) {
        (Some("EC"), Some("P-256")) => {
            let x = decode_coordinate(key, "x", 32)?;
            let y = decode_coordinate(key, "y", 32)?;
            let thumbprint = sha256_base64url(&format!(
                r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#,
                URL_SAFE_NO_PAD.encode(&x),
                URL_SAFE_NO_PAD.encode(&y)
            ));
            let mut point = vec![0x04];
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            Ok((PublicKey::P256(point), thumbprint))
        }
        (Some("OKP"), Some("Ed25519")) => {
            let x = decode_coordinate(key, "x", 32)?;
            let thumbprint = sha256_base64url(&format!(
                r#"{{"crv":"Ed25519","kty":"OKP","x":"{}"}}"#,
                URL_SAFE_NO_PAD.encode(&x)
            ));
            Ok((PublicKey::Ed25519(x), thumbprint))
        }
        _ => Err(KeyError(String::from(
            "only EC P-256 and OKP Ed25519 keys are accepted",
        ))),
    }
}

impl Jwks {
    pub fn from_json(text: &str) -> Result<Jwks, KeyError> {
        let document: Value =
            serde_json::from_str(text).map_err(|error| KeyError(format!("not JSON: {error}")))?;
        let Some(keys) = document.get("keys").and_then(Value::as_array) else {
            return Err(KeyError(String::from("a JWKS needs a keys array")));
        };
        let mut jwks = Jwks::default();
        for (position, key) in keys.iter().enumerate() {
            let (public_key, thumbprint) = public_key_of(key)
                .map_err(|KeyError(reason)| KeyError(format!("key {position}: {reason}")))?;
            let key_id = member(key, "kid").map(String::from).unwrap_or(thumbprint);
            let max_installations = match key.get("max_installations") {
                None => None,
                Some(limit) => {
                    Some(limit.as_u64().filter(|limit| *limit > 0).ok_or_else(|| {
                        KeyError(format!(
                            "key {position}: max_installations must be a whole number of at least 1"
                        ))
                    })?)
                }
            };
            let entry = JwksKey {
                public_key,
                max_installations,
            };
            if jwks.keys.insert(key_id.clone(), entry).is_some() {
                return Err(KeyError(format!(
                    "key {position}: key id {key_id} appears twice"
                )));
            }
        }
        Ok(jwks)
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn max_installations(&self, key_id: &str) -> Option<u64> {
        self.keys.get(key_id).and_then(|key| key.max_installations)
    }

    pub fn limits(&self) -> impl Iterator<Item = (&str, u64)> {
        self.keys
            .iter()
            .filter_map(|(key_id, key)| key.max_installations.map(|limit| (key_id.as_str(), limit)))
    }

    pub fn verify(&self, token: &str) -> Result<Map<String, Value>, JwtError> {
        self.verify_with_key_id(token).map(|(_, claims)| claims)
    }

    pub fn verify_with_key_id(
        &self,
        token: &str,
    ) -> Result<(String, Map<String, Value>), JwtError> {
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(JwtError::Malformed);
        };
        let decode = |part: &str| {
            URL_SAFE_NO_PAD
                .decode(part)
                .map_err(|_| JwtError::Malformed)
        };
        let header_value: Value = serde_json::from_slice(&decode(header)?)
            .map_err(|error| JwtError::Header(error.to_string()))?;
        if header_value.get("crit").is_some() {
            return Err(JwtError::Header(String::from(
                "critical header parameters are not accepted",
            )));
        }
        let algorithm =
            member(&header_value, "alg").ok_or_else(|| JwtError::Header(String::from("no alg")))?;
        let key_id =
            member(&header_value, "kid").ok_or_else(|| JwtError::Header(String::from("no kid")))?;
        let key = self
            .keys
            .get(key_id)
            .ok_or_else(|| JwtError::UnknownKey(String::from(key_id)))?;
        let signed = &token.as_bytes()[..header.len() + 1 + payload.len()];
        let signature = decode(signature)?;
        let verified = match (algorithm, &key.public_key) {
            ("ES256", PublicKey::P256(point)) => {
                UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point).verify(signed, &signature)
            }
            ("EdDSA", PublicKey::Ed25519(public_key)) => {
                UnparsedPublicKey::new(&ED25519, public_key).verify(signed, &signature)
            }
            (other, _) => return Err(JwtError::Algorithm(String::from(other))),
        };
        verified.map_err(|_| JwtError::Signature)?;
        match serde_json::from_slice(&decode(payload)?) {
            Ok(Value::Object(claims)) => Ok((String::from(key_id), claims)),
            _ => Err(JwtError::Claims),
        }
    }
}

pub struct SigningKey {
    key_pair: EcdsaKeyPair,
    key_id: String,
    random: SystemRandom,
}

impl SigningKey {
    pub fn from_jwk(text: &str) -> Result<SigningKey, KeyError> {
        let key: Value =
            serde_json::from_str(text).map_err(|error| KeyError(format!("not JSON: {error}")))?;
        if member(&key, "kty") != Some("EC") || member(&key, "crv") != Some("P-256") {
            return Err(KeyError(String::from(
                "the provisioner key must be an EC P-256 JWK",
            )));
        }
        if key.get("ciphertext").is_some() || key.get("protected").is_some() {
            return Err(KeyError(String::from(
                "the provisioner key is an encrypted JWE; store the decrypted JWK",
            )));
        }
        let private_key = decode_coordinate(&key, "d", 32)?;
        let (PublicKey::P256(point), thumbprint) = public_key_of(&key)? else {
            unreachable!("an EC P-256 JWK yields a P-256 key")
        };
        let random = SystemRandom::new();
        let key_pair = EcdsaKeyPair::from_private_key_and_public_key(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            &private_key,
            &point,
            &random,
        )
        .map_err(|error| {
            KeyError(format!(
                "the private key does not match its public key: {error}"
            ))
        })?;
        let key_id = member(&key, "kid").map(String::from).unwrap_or(thumbprint);
        Ok(SigningKey {
            key_pair,
            key_id,
            random,
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn sign(&self, claims: &Value) -> Result<String, KeyError> {
        let header = json!({ "alg": "ES256", "kid": self.key_id, "typ": "JWT" });
        let mut token = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = self
            .key_pair
            .sign(&self.random, token.as_bytes())
            .map_err(|error| KeyError(format!("signing failed: {error}")))?;
        token.push('.');
        token.push_str(&URL_SAFE_NO_PAD.encode(signature.as_ref()));
        Ok(token)
    }

    pub fn public_jwk(&self) -> Value {
        use ring::signature::KeyPair;
        let point = self.key_pair.public_key().as_ref();
        json!({
            "kty": "EC",
            "crv": "P-256",
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
            "kid": self.key_id,
            "use": "sig",
            "alg": "ES256",
        })
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair};
    use serde_json::{Value, json};

    pub(crate) fn private_jwk() -> String {
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let key_pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                .unwrap();
        let point = key_pair.public_key().as_ref();
        json!({
            "kty": "EC",
            "crv": "P-256",
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
            "d": URL_SAFE_NO_PAD.encode(&pkcs8.as_ref()[36..68]),
        })
        .to_string()
    }

    pub(crate) struct EdwardsSigner {
        key_pair: Ed25519KeyPair,
        pub(crate) key_id: String,
    }

    impl EdwardsSigner {
        pub(crate) fn new(key_id: &str) -> EdwardsSigner {
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
            EdwardsSigner {
                key_pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(),
                key_id: String::from(key_id),
            }
        }

        pub(crate) fn public_jwk(&self) -> Value {
            json!({
                "kty": "OKP",
                "crv": "Ed25519",
                "x": URL_SAFE_NO_PAD.encode(self.key_pair.public_key().as_ref()),
                "kid": self.key_id,
            })
        }

        pub(crate) fn sign(&self, claims: &Value) -> String {
            let header = json!({ "alg": "EdDSA", "kid": self.key_id, "typ": "JWT" });
            let signed = format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(header.to_string()),
                URL_SAFE_NO_PAD.encode(claims.to_string())
            );
            let signature = self.key_pair.sign(signed.as_bytes());
            format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{EdwardsSigner, private_jwk};
    use super::*;

    #[test]
    fn signs_es256_tokens_its_public_key_verifies() {
        let key = SigningKey::from_jwk(&private_jwk()).unwrap();
        assert_eq!(key.key_id().len(), 43);
        let jwks = Jwks::from_json(&json!({ "keys": [key.public_jwk()] }).to_string()).unwrap();
        let token = key.sign(&json!({ "sub": "node", "number": 7 })).unwrap();
        let claims = jwks.verify(&token).unwrap();
        assert_eq!(claims["sub"], "node");
        let mut tampered = token.clone().into_bytes();
        let position = token.find('.').unwrap() + 3;
        tampered[position] = if tampered[position] == b'A' {
            b'B'
        } else {
            b'A'
        };
        assert!(
            jwks.verify(std::str::from_utf8(&tampered).unwrap())
                .is_err()
        );
    }

    #[test]
    fn keeps_a_declared_key_id_and_otherwise_uses_the_thumbprint() {
        let mut jwk: Value = serde_json::from_str(&private_jwk()).unwrap();
        let derived = SigningKey::from_jwk(&jwk.to_string())
            .unwrap()
            .key_id()
            .to_string();
        jwk["kid"] = Value::from("nightfall");
        assert_eq!(
            SigningKey::from_jwk(&jwk.to_string()).unwrap().key_id(),
            "nightfall"
        );
        let public = json!({ "kty": "EC", "crv": "P-256", "x": jwk["x"], "y": jwk["y"] });
        let jwks = Jwks::from_json(&json!({ "keys": [public] }).to_string()).unwrap();
        assert!(jwks.keys.contains_key(&derived));
    }

    #[test]
    fn verifies_eddsa_and_refuses_algorithm_confusion() {
        let edwards = EdwardsSigner::new("installer");
        let es256 = SigningKey::from_jwk(&private_jwk()).unwrap();
        let mut es256_public = es256.public_jwk();
        es256_public["kid"] = Value::from("installer-ec");
        let jwks =
            Jwks::from_json(&json!({ "keys": [edwards.public_jwk(), es256_public] }).to_string())
                .unwrap();
        assert_eq!(jwks.len(), 2);
        assert_eq!(
            jwks.verify(&edwards.sign(&json!({ "a": 1 }))).unwrap()["a"],
            1
        );

        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"ES256","kid":"installer"}"#);
        let forged = format!(
            "{header}.{}.{}",
            URL_SAFE_NO_PAD.encode("{}"),
            URL_SAFE_NO_PAD.encode([0u8; 64])
        );
        assert_eq!(
            jwks.verify(&forged),
            Err(JwtError::Algorithm(String::from("ES256")))
        );

        let none = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"none","kid":"installer"}"#),
            URL_SAFE_NO_PAD.encode("{}")
        );
        assert_eq!(
            jwks.verify(&none),
            Err(JwtError::Algorithm(String::from("none")))
        );
        assert_eq!(jwks.verify("a.b"), Err(JwtError::Malformed));
        let unknown = format!(
            "{}.{}.AA",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"EdDSA","kid":"other"}"#),
            URL_SAFE_NO_PAD.encode("{}")
        );
        assert_eq!(
            jwks.verify(&unknown),
            Err(JwtError::UnknownKey(String::from("other")))
        );
    }

    #[test]
    fn reads_the_installation_cap_of_each_key() {
        let capped = EdwardsSigner::new("factory-2026");
        let open = EdwardsSigner::new("factory-2025");
        let mut capped_public = capped.public_jwk();
        capped_public["max_installations"] = Value::from(500);
        let jwks =
            Jwks::from_json(&json!({ "keys": [capped_public, open.public_jwk()] }).to_string())
                .unwrap();
        assert_eq!(jwks.max_installations("factory-2026"), Some(500));
        assert_eq!(jwks.max_installations("factory-2025"), None);
        assert_eq!(
            jwks.limits().collect::<Vec<_>>(),
            vec![("factory-2026", 500)]
        );
        let (key_id, claims) = jwks
            .verify_with_key_id(&capped.sign(&json!({ "a": 1 })))
            .unwrap();
        assert_eq!(
            (key_id.as_str(), &claims["a"]),
            ("factory-2026", &Value::from(1))
        );
        for refused in [
            Value::from(0),
            Value::from(-1),
            Value::from("500"),
            Value::from(1.5),
        ] {
            let mut public = capped.public_jwk();
            public["max_installations"] = refused.clone();
            assert!(
                Jwks::from_json(&json!({ "keys": [public] }).to_string()).is_err(),
                "accepted {refused}"
            );
        }
    }

    #[test]
    fn refuses_encrypted_and_wrong_curve_keys() {
        assert!(
            SigningKey::from_jwk(r#"{"protected":"x","ciphertext":"y","kty":"EC","crv":"P-256"}"#)
                .is_err()
        );
        assert!(
            SigningKey::from_jwk(r#"{"kty":"OKP","crv":"Ed25519","x":"AA","d":"AA"}"#).is_err()
        );
        assert!(Jwks::from_json(r#"{"keys":[{"kty":"RSA","n":"AA","e":"AQAB"}]}"#).is_err());
    }
}
