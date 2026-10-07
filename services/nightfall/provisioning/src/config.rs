use std::path::{Path, PathBuf};
use std::time::Duration;

use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;

use crate::credential::{FleetTokens, InstallTokenKeys};
use crate::identity::DeviceIdKey;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {reason}")]
    Invalid { path: PathBuf, reason: String },
}

impl ConfigError {
    pub fn read(path: &Path, source: std::io::Error) -> ConfigError {
        ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }
    }

    pub fn invalid(path: &Path, reason: impl Into<String>) -> ConfigError {
        ConfigError::Invalid {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }
}

pub fn load_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, ConfigError> {
    let pem = std::fs::read(path).map_err(|source| ConfigError::read(path, source))?;
    let certificates = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            ConfigError::invalid(path, format!("not a PEM certificate bundle: {error}"))
        })?;
    if certificates.is_empty() {
        return Err(ConfigError::invalid(path, "the file holds no certificate"));
    }
    Ok(certificates)
}

pub struct ProvisioningConfig {
    pub instance: String,
    pub fleet_tokens: FleetTokens,
    pub install_token_keys: InstallTokenKeys,
    pub device_id_key: DeviceIdKey,
    pub fleet_client_roots: Vec<CertificateDer<'static>>,
    pub challenge_ttl: Duration,
    pub challenge_capacity: usize,
    pub renew_grace: Duration,
    pub certificate_lifetime: Duration,
    pub enrollments_per_second: u32,
    pub enrollments_per_second_per_credential: u32,
    pub enrollment_alert_per_minute: u64,
}

pub struct ProvisioningFiles<'a> {
    pub instance: &'a str,
    pub fleet_tokens_file: &'a Path,
    pub install_token_keys: &'a Path,
    pub device_id_key_file: &'a Path,
    pub fleet_client_ca: &'a Path,
}

impl ProvisioningConfig {
    pub fn load(paths: &ProvisioningFiles<'_>) -> Result<ProvisioningConfig, ConfigError> {
        Ok(ProvisioningConfig {
            instance: String::from(paths.instance),
            fleet_tokens: FleetTokens::load(paths.fleet_tokens_file)?,
            install_token_keys: InstallTokenKeys::load(paths.install_token_keys)?,
            device_id_key: DeviceIdKey::load(paths.device_id_key_file)?,
            fleet_client_roots: load_certificates(paths.fleet_client_ca)?,
            challenge_ttl: Duration::from_millis(300_000),
            challenge_capacity: 65_536,
            renew_grace: Duration::from_secs(2160 * 3600),
            certificate_lifetime: Duration::from_secs(168 * 3600),
            enrollments_per_second: 50,
            enrollments_per_second_per_credential: 10,
            enrollment_alert_per_minute: 600,
        })
    }
}
