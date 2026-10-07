use std::path::Path;

use ring::hmac;

use crate::entry::hex;
use crate::signing::KeyError;

pub const MINIMUM_KEY_BYTES: usize = 32;

pub struct ParamKey(hmac::Key);

impl ParamKey {
    pub fn load(path: &Path) -> Result<ParamKey, KeyError> {
        let text = std::fs::read_to_string(path).map_err(|source| KeyError::Read {
            path: path.display().to_string(),
            source,
        })?;
        ParamKey::from_hex(&text)
            .map_err(|error| KeyError::Invalid(format!("{}: {error}", path.display())))
    }

    pub fn from_hex(text: &str) -> Result<ParamKey, KeyError> {
        let bytes = decode_hex_key(text)?;
        Ok(ParamKey(hmac::Key::new(hmac::HMAC_SHA256, &bytes)))
    }

    pub fn hash(&self, canonical_params: &[u8]) -> String {
        hex(hmac::sign(&self.0, canonical_params).as_ref())
    }
}

pub fn decode_hex_key(text: &str) -> Result<Vec<u8>, KeyError> {
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if !trimmed.len().is_multiple_of(2) || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(KeyError::Invalid(String::from(
            "the key file must hold the key as hexadecimal digits",
        )));
    }
    let bytes: Vec<u8> = (0..trimmed.len())
        .step_by(2)
        .map(|position| {
            u8::from_str_radix(&trimmed[position..position + 2], 16).expect("checked hex digits")
        })
        .collect();
    if bytes.len() < MINIMUM_KEY_BYTES {
        return Err(KeyError::Invalid(format!(
            "the key is {} bytes; it needs at least {MINIMUM_KEY_BYTES}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_pythons_hmac_sha256() {
        let key = ParamKey::from_hex(&format!("{}\n", "0b".repeat(32))).unwrap();
        assert_eq!(
            key.hash(b"Hi There"),
            "198a607eb44bfbc69903a0f1cf2bbdc5ba0aa3f3d9ae3c1c7a3b1696a0b68cf7"
        );
    }

    #[test]
    fn refuses_short_and_malformed_keys() {
        assert!(ParamKey::from_hex(&"0b".repeat(31)).is_err());
        assert!(ParamKey::from_hex(&"zz".repeat(32)).is_err());
        assert!(ParamKey::from_hex(&"0".repeat(65)).is_err());
    }
}
