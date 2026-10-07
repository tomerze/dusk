use std::path::Path;

use ring::hmac;
use ring::rand::SecureRandom;

use crate::config::ConfigError;

pub const DEVICE_ID_LABEL: &[u8] = b"dusk-device-v1";
pub const HARDWARE_FINGERPRINT_BYTES: usize = 32;
pub const MINIMUM_KEY_BYTES: usize = 32;

const DEVICE_PREFIX: &str = "urn:dusk:device:";
const INSTALLATION_PREFIX: &str = "urn:dusk:installation:";
const TENANT_PREFIX: &str = "urn:dusk:tenant:";

pub struct DeviceIdKey(hmac::Key);

impl DeviceIdKey {
    pub fn load(path: &Path) -> Result<DeviceIdKey, ConfigError> {
        let text =
            std::fs::read_to_string(path).map_err(|source| ConfigError::read(path, source))?;
        DeviceIdKey::from_hex(&text).map_err(|reason| ConfigError::invalid(path, reason))
    }

    pub fn from_hex(text: &str) -> Result<DeviceIdKey, String> {
        Ok(DeviceIdKey(hmac::Key::new(
            hmac::HMAC_SHA256,
            &decode_hex_key(text)?,
        )))
    }

    pub fn device_id(&self, hardware_fingerprint: &[u8]) -> String {
        let mut context = hmac::Context::with_key(&self.0);
        context.update(DEVICE_ID_LABEL);
        context.update(hardware_fingerprint);
        hex::encode(&context.sign().as_ref()[..16])
    }
}

pub fn decode_hex_key(text: &str) -> Result<Vec<u8>, String> {
    let bytes = hex::decode(text.trim_end_matches(['\n', '\r']))
        .map_err(|_| String::from("the key file must hold the key as hexadecimal digits"))?;
    if bytes.len() < MINIMUM_KEY_BYTES {
        return Err(format!(
            "the key is {} bytes; it needs at least {MINIMUM_KEY_BYTES}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

pub(crate) fn new_installation_id(
    random: &dyn SecureRandom,
) -> Result<String, ring::error::Unspecified> {
    let mut bytes = [0u8; 16];
    random.fill(&mut bytes)?;
    Ok(hex::encode(bytes))
}

pub fn is_identifier(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn is_tenant(text: &str) -> bool {
    (1..=63).contains(&text.len())
        && text
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

pub fn device_uri(device_id: &str) -> String {
    format!("{DEVICE_PREFIX}{device_id}")
}

pub fn installation_uri(installation_id: &str) -> String {
    format!("{INSTALLATION_PREFIX}{installation_id}")
}

pub fn tenant_uri(tenant: &str) -> String {
    format!("{TENANT_PREFIX}{tenant}")
}

pub fn common_name(device_id: &str, installation_id: &str) -> String {
    format!("{device_id}.{installation_id}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateIdentity {
    pub device_id: String,
    pub installation_id: String,
    pub tenant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("the certificate names {0} device ids")]
    DeviceCount(usize),
    #[error("the certificate names {0} installation ids")]
    InstallationCount(usize),
    #[error("the certificate names {0} tenants")]
    TenantCount(usize),
    #[error("the certificate carries an unexpected URI {0}")]
    UnexpectedUri(String),
}

pub fn identity_from_uris<'a>(
    uris: impl IntoIterator<Item = &'a str>,
) -> Result<CertificateIdentity, IdentityError> {
    let mut devices = Vec::new();
    let mut installations = Vec::new();
    let mut tenants = Vec::new();
    for uri in uris {
        if let Some(device_id) = uri
            .strip_prefix(DEVICE_PREFIX)
            .filter(|id| is_identifier(id))
        {
            devices.push(String::from(device_id));
        } else if let Some(installation_id) = uri
            .strip_prefix(INSTALLATION_PREFIX)
            .filter(|id| is_identifier(id))
        {
            installations.push(String::from(installation_id));
        } else if let Some(tenant) = uri
            .strip_prefix(TENANT_PREFIX)
            .filter(|tenant| is_tenant(tenant))
        {
            tenants.push(String::from(tenant));
        } else {
            return Err(IdentityError::UnexpectedUri(String::from(uri)));
        }
    }
    if devices.len() != 1 {
        return Err(IdentityError::DeviceCount(devices.len()));
    }
    if installations.len() != 1 {
        return Err(IdentityError::InstallationCount(installations.len()));
    }
    if tenants.len() > 1 {
        return Err(IdentityError::TenantCount(tenants.len()));
    }
    Ok(CertificateIdentity {
        device_id: devices.remove(0),
        installation_id: installations.remove(0),
        tenant: tenants.pop(),
    })
}

#[cfg(test)]
mod tests {
    use ring::rand::SystemRandom;

    use super::*;

    #[test]
    fn derives_the_device_id_from_the_label_and_the_fingerprint() {
        let key = DeviceIdKey::from_hex(&format!("{}\n", "0b".repeat(32))).unwrap();
        let fingerprint = [7u8; HARDWARE_FINGERPRINT_BYTES];
        let expected = {
            let mut message = DEVICE_ID_LABEL.to_vec();
            message.extend_from_slice(&fingerprint);
            let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &[0x0b; 32]), &message);
            hex::encode(&tag.as_ref()[..16])
        };
        assert_eq!(key.device_id(&fingerprint), expected);
        assert!(is_identifier(&key.device_id(&fingerprint)));
        assert_ne!(key.device_id(&fingerprint), key.device_id(&[8u8; 32]));
    }

    #[test]
    fn refuses_short_and_malformed_keys() {
        assert!(DeviceIdKey::from_hex(&"0b".repeat(31)).is_err());
        assert!(DeviceIdKey::from_hex(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn assigns_random_installation_ids() {
        let random = SystemRandom::new();
        let first = new_installation_id(&random).unwrap();
        assert!(is_identifier(&first));
        assert_ne!(first, new_installation_id(&random).unwrap());
    }

    #[test]
    fn validates_tenants() {
        assert!(is_tenant("retail-eu"));
        assert!(is_tenant(&"a".repeat(63)));
        assert!(!is_tenant(&"a".repeat(64)));
        assert!(!is_tenant(""));
        assert!(!is_tenant("Retail"));
        assert!(!is_tenant("retail_eu"));
    }

    #[test]
    fn reads_the_identity_from_san_uris() {
        let device = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13";
        let installation = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70";
        let identity = identity_from_uris([
            device_uri(device).as_str(),
            installation_uri(installation).as_str(),
            tenant_uri("retail-eu").as_str(),
        ])
        .unwrap();
        assert_eq!(identity.device_id, device);
        assert_eq!(identity.installation_id, installation);
        assert_eq!(identity.tenant.as_deref(), Some("retail-eu"));
        assert_eq!(
            identity_from_uris([device_uri(device).as_str()]),
            Err(IdentityError::InstallationCount(0))
        );
        assert_eq!(
            identity_from_uris([
                device_uri(device).as_str(),
                device_uri(device).as_str(),
                installation_uri(installation).as_str()
            ]),
            Err(IdentityError::DeviceCount(2))
        );
        assert!(matches!(
            identity_from_uris([
                "urn:dusk:device:3F9C",
                installation_uri(installation).as_str()
            ]),
            Err(IdentityError::UnexpectedUri(_))
        ));
    }
}
