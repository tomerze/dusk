use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::config::ConfigError;
use crate::identity::is_tenant;
use crate::jwt::{Jwks, JwtError};

pub const CLOCK_SKEW_SECONDS: u64 = 60;
const MAXIMUM_CLAIM_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    FleetToken,
    InstallToken,
    Certificate,
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .try_into()
        .expect("SHA-256 is 32 bytes")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetToken {
    pub name: String,
    pub value_sha256: [u8; 32],
    pub tenant: Option<String>,
    pub retired: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FleetTokens {
    tokens: Vec<FleetToken>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FleetTokensFile {
    #[serde(default)]
    token: Vec<FleetTokenEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FleetTokenEntry {
    name: String,
    value_sha256: String,
    tenant: Option<String>,
    #[serde(default)]
    retired: bool,
}

fn equal_in_constant_time(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let difference = left
        .iter()
        .zip(right)
        .fold(0u8, |accumulated, (left, right)| {
            accumulated | (left ^ right)
        });
    std::hint::black_box(difference) == 0
}

impl FleetTokens {
    pub fn load(path: &Path) -> Result<FleetTokens, ConfigError> {
        let text =
            std::fs::read_to_string(path).map_err(|source| ConfigError::read(path, source))?;
        FleetTokens::from_toml(&text).map_err(|reason| ConfigError::invalid(path, reason))
    }

    pub fn from_toml(text: &str) -> Result<FleetTokens, String> {
        let file: FleetTokensFile = toml::from_str(text).map_err(|error| error.to_string())?;
        let mut names = HashSet::new();
        let mut digests = HashSet::new();
        let mut tokens = Vec::with_capacity(file.token.len());
        for entry in file.token {
            if entry.name.is_empty() || entry.name.len() > MAXIMUM_CLAIM_BYTES {
                return Err(String::from("every token needs a name of 1 to 256 bytes"));
            }
            let value_sha256: [u8; 32] = hex::decode(&entry.value_sha256)
                .ok()
                .filter(|_| {
                    entry
                        .value_sha256
                        .bytes()
                        .all(|byte| !byte.is_ascii_uppercase())
                })
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| {
                    format!(
                        "token {}: value_sha256 is not 64 lowercase hex digits",
                        entry.name
                    )
                })?;
            if let Some(tenant) = &entry.tenant
                && !is_tenant(tenant)
            {
                return Err(format!(
                    "token {}: tenant {tenant} does not match [a-z0-9-]{{1,63}}",
                    entry.name
                ));
            }
            if !names.insert(entry.name.clone()) {
                return Err(format!("token {} appears twice", entry.name));
            }
            if !digests.insert(value_sha256) {
                return Err(format!(
                    "token {} has the value of another entry",
                    entry.name
                ));
            }
            tokens.push(FleetToken {
                name: entry.name,
                value_sha256,
                tenant: entry.tenant,
                retired: entry.retired,
            });
        }
        Ok(FleetTokens { tokens })
    }

    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    pub fn retired(&self) -> usize {
        self.tokens.iter().filter(|token| token.retired).count()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FleetToken> {
        self.tokens.iter()
    }

    pub fn find(&self, presented: &str) -> Option<&FleetToken> {
        let digest = sha256(presented.as_bytes());
        let mut found = None;
        for token in &self.tokens {
            let matches = equal_in_constant_time(&token.value_sha256, &digest);
            found = if matches { Some(token) } else { found };
        }
        found
    }
}

#[derive(Debug, Clone, Default)]
pub struct InstallTokenKeys {
    jwks: Jwks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallToken {
    pub key_id: String,
    pub issuer: String,
    pub token_id: String,
    pub subject: String,
    pub tenant: Option<String>,
    pub expires_at: u64,
}

impl InstallToken {
    pub fn one_time_token_id(&self) -> String {
        let mut material = Vec::new();
        for part in [
            b"dusk-install-token".as_slice(),
            self.issuer.as_bytes(),
            self.token_id.as_bytes(),
        ] {
            let length = u32::try_from(part.len()).expect("a claim is at most 256 bytes");
            material.extend_from_slice(&length.to_be_bytes());
            material.extend_from_slice(part);
        }
        hex::encode(sha256(&material))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallTokenError {
    #[error(transparent)]
    Signature(#[from] JwtError),
    #[error("claim {0} is missing or not valid")]
    Claim(&'static str),
    #[error("the token expired")]
    Expired,
    #[error("the token is not valid yet")]
    NotYetValid,
}

impl InstallTokenKeys {
    pub fn load(path: &Path) -> Result<InstallTokenKeys, ConfigError> {
        let text =
            std::fs::read_to_string(path).map_err(|source| ConfigError::read(path, source))?;
        InstallTokenKeys::from_json(&text)
            .map_err(|error| ConfigError::invalid(path, error.to_string()))
    }

    pub fn from_json(text: &str) -> Result<InstallTokenKeys, crate::jwt::KeyError> {
        Ok(InstallTokenKeys {
            jwks: Jwks::from_json(text)?,
        })
    }

    pub fn len(&self) -> usize {
        self.jwks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jwks.is_empty()
    }

    pub fn verify(
        &self,
        token: &str,
        now_unix_seconds: u64,
    ) -> Result<InstallToken, InstallTokenError> {
        let (key_id, claims) = self.jwks.verify_with_key_id(token)?;
        let text = |name: &'static str| {
            claims
                .get(name)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= MAXIMUM_CLAIM_BYTES)
                .map(String::from)
                .ok_or(InstallTokenError::Claim(name))
        };
        let issuer = text("iss")?;
        let token_id = text("jti")?;
        let subject = text("sub")?;
        let expires_at = claims
            .get("exp")
            .and_then(Value::as_u64)
            .ok_or(InstallTokenError::Claim("exp"))?;
        if expires_at.saturating_add(CLOCK_SKEW_SECONDS) <= now_unix_seconds {
            return Err(InstallTokenError::Expired);
        }
        match claims.get("nbf") {
            None => {}
            Some(not_before) => {
                let not_before = not_before.as_u64().ok_or(InstallTokenError::Claim("nbf"))?;
                if not_before > now_unix_seconds.saturating_add(CLOCK_SKEW_SECONDS) {
                    return Err(InstallTokenError::NotYetValid);
                }
            }
        }
        let tenant = match claims.get("tenant") {
            None | Some(Value::Null) => None,
            Some(Value::String(tenant)) if is_tenant(tenant) => Some(tenant.clone()),
            Some(_) => return Err(InstallTokenError::Claim("tenant")),
        };
        Ok(InstallToken {
            key_id,
            issuer,
            token_id,
            subject,
            tenant,
            expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::jwt::fixtures::EdwardsSigner;

    const SUBJECT: &str = "kiosk-0042";

    fn tokens_file() -> String {
        format!(
            "[[token]]\nname = \"retail-eu-2026\"\nvalue_sha256 = \"{}\"\ntenant = \"retail-eu\"\n\n[[token]]\nname = \"lab\"\nvalue_sha256 = \"{}\"\n",
            hex::encode(sha256(b"retail secret")),
            hex::encode(sha256(b"lab secret"))
        )
    }

    #[test]
    fn finds_a_fleet_token_by_its_digest() {
        let tokens = FleetTokens::from_toml(&tokens_file()).unwrap();
        assert_eq!(tokens.len(), 2);
        let retail = tokens.find("retail secret").unwrap();
        assert_eq!(retail.name, "retail-eu-2026");
        assert_eq!(retail.tenant.as_deref(), Some("retail-eu"));
        assert_eq!(tokens.find("lab secret").unwrap().tenant, None);
        assert!(tokens.find("retail secret ").is_none());
        assert!(tokens.find("").is_none());
        assert!(FleetTokens::from_toml("").unwrap().is_empty());
    }

    #[test]
    fn refuses_a_malformed_fleet_tokens_file() {
        let digest = hex::encode(sha256(b"x"));
        let refused = [
            format!(
                "[[token]]\nname = \"a\"\nvalue_sha256 = \"{}\"\n",
                digest.to_uppercase()
            ),
            String::from("[[token]]\nname = \"a\"\nvalue_sha256 = \"abcd\"\n"),
            format!("[[token]]\nname = \"a\"\nvalue_sha256 = \"{digest}\"\ntenant = \"Retail\"\n"),
            format!(
                "[[token]]\nname = \"a\"\nvalue_sha256 = \"{digest}\"\n[[token]]\nname = \"a\"\nvalue_sha256 = \"{}\"\n",
                hex::encode(sha256(b"y"))
            ),
            format!(
                "[[token]]\nname = \"a\"\nvalue_sha256 = \"{digest}\"\n[[token]]\nname = \"b\"\nvalue_sha256 = \"{digest}\"\n"
            ),
            format!("[[token]]\nname = \"\"\nvalue_sha256 = \"{digest}\"\n"),
            format!("[[token]]\nname = \"a\"\nvalue = \"plain\"\nvalue_sha256 = \"{digest}\"\n"),
            format!("[[token]]\nname = \"a\"\nvalue_sha256 = \"{digest}\"\nretired = \"yes\"\n"),
        ];
        for text in refused {
            assert!(FleetTokens::from_toml(&text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn reads_the_retirement_of_a_fleet_token() {
        let tokens = FleetTokens::from_toml(&format!(
            "[[token]]\nname = \"batch-7\"\nvalue_sha256 = \"{}\"\n\n[[token]]\nname = \"old\"\nvalue_sha256 = \"{}\"\nretired = true\n",
            hex::encode(sha256(b"batch secret")),
            hex::encode(sha256(b"old secret"))
        ))
        .unwrap();
        assert!(!tokens.find("batch secret").unwrap().retired);
        assert!(tokens.find("old secret").unwrap().retired);
        assert_eq!(tokens.retired(), 1);
    }

    fn keys(signer: &EdwardsSigner) -> InstallTokenKeys {
        InstallTokenKeys::from_json(&json!({ "keys": [signer.public_jwk()] }).to_string()).unwrap()
    }

    #[test]
    fn verifies_an_install_token() {
        let signer = EdwardsSigner::new("factory-2026");
        let token = signer.sign(&json!({
            "iss": "factory", "sub": SUBJECT, "jti": "batch-7-unit-42", "exp": 2_000_000_000u64,
            "nbf": 1_000_000_000u64, "tenant": "retail-eu",
        }));
        let verified = keys(&signer).verify(&token, 1_791_278_043).unwrap();
        assert_eq!(verified.subject, SUBJECT);
        assert_eq!(verified.key_id, "factory-2026");
        assert_eq!(verified.issuer, "factory");
        assert_eq!(verified.tenant.as_deref(), Some("retail-eu"));
        let mut material = b"\x00\x00\x00\x12dusk-install-token".to_vec();
        material.extend_from_slice(b"\x00\x00\x00\x07factory\x00\x00\x00\x0fbatch-7-unit-42");
        assert_eq!(verified.one_time_token_id(), hex::encode(sha256(&material)));
    }

    #[test]
    fn gives_every_issuer_and_token_id_pair_its_own_one_time_token_id() {
        let token = |issuer: &str, token_id: &str| InstallToken {
            key_id: String::from("factory-2026"),
            issuer: String::from(issuer),
            token_id: String::from(token_id),
            subject: String::from(SUBJECT),
            tenant: None,
            expires_at: 0,
        };
        assert_ne!(
            token("factory", "1-2").one_time_token_id(),
            token("factory1", "-2").one_time_token_id()
        );
        assert_eq!(
            token("factory", "1-2").one_time_token_id(),
            token("factory", "1-2").one_time_token_id()
        );
    }

    #[test]
    fn refuses_install_tokens_with_bad_claims() {
        let signer = EdwardsSigner::new("factory-2026");
        let keys = keys(&signer);
        let now = 1_791_278_043u64;
        let base = json!({ "iss": "factory", "sub": SUBJECT, "jti": "j", "exp": now + 600 });
        let with = |name: &str, value: Value| {
            let mut claims = base.clone();
            claims[name] = value;
            keys.verify(&signer.sign(&claims), now)
        };
        assert_eq!(
            with("sub", Value::from("")),
            Err(InstallTokenError::Claim("sub"))
        );
        assert_eq!(
            with("sub", Value::from(7)),
            Err(InstallTokenError::Claim("sub"))
        );
        assert_eq!(
            with("exp", Value::from(now - 61)),
            Err(InstallTokenError::Expired)
        );
        assert!(with("exp", Value::from(now - 59)).is_ok());
        assert_eq!(
            with("nbf", Value::from(now + 120)),
            Err(InstallTokenError::NotYetValid)
        );
        assert_eq!(
            with("tenant", Value::from("Retail")),
            Err(InstallTokenError::Claim("tenant"))
        );
        assert_eq!(
            with("jti", Value::from("")),
            Err(InstallTokenError::Claim("jti"))
        );
        assert_eq!(
            with("iss", Value::Null),
            Err(InstallTokenError::Claim("iss"))
        );
        assert_eq!(
            with("exp", Value::from("soon")),
            Err(InstallTokenError::Claim("exp"))
        );
        let stranger = EdwardsSigner::new("factory-2026");
        assert_eq!(
            keys.verify(&stranger.sign(&base), now),
            Err(InstallTokenError::Signature(JwtError::Signature))
        );
        assert!(
            InstallTokenKeys::from_json(r#"{"keys":[]}"#)
                .unwrap()
                .verify(&signer.sign(&base), now)
                .is_err()
        );
    }
}
