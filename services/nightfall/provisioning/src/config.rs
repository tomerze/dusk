use std::path::{Path, PathBuf};

use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;

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
