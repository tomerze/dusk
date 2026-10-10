use anyhow::bail;
use rustls_pki_types::CertificateDer;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

pub const DEVICE_PREFIX: &str = "urn:dusk:device:";
pub const INSTALLATION_PREFIX: &str = "urn:dusk:installation:";
pub const PRINCIPAL_PREFIX: &str = "urn:dusk:principal:";
pub const MAXIMUM_PRINCIPAL_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerCertificate {
    pub uris: Vec<String>,
    pub other_names: usize,
    pub fingerprint: String,
    pub not_after_unix: i64,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    nightfall_ledger::entry::sha256_hex(bytes)
}

pub fn inspect(certificate: &CertificateDer<'_>) -> anyhow::Result<PeerCertificate> {
    let (_, parsed) = X509Certificate::from_der(certificate.as_ref())
        .map_err(|error| anyhow::anyhow!("parse the peer certificate: {error}"))?;
    let mut uris = Vec::new();
    let mut other_names = 0usize;
    if let Some(alternative) = parsed
        .subject_alternative_name()
        .map_err(|error| anyhow::anyhow!("read the subject alternative names: {error}"))?
    {
        for name in &alternative.value.general_names {
            match name {
                GeneralName::URI(uri) => uris.push(uri.to_string()),
                _ => other_names += 1,
            }
        }
    }
    Ok(PeerCertificate {
        uris,
        other_names,
        fingerprint: sha256_hex(certificate.as_ref()),
        not_after_unix: parsed.validity().not_after.timestamp(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeCertificate {
    pub device_id: String,
    pub installation_id: String,
    pub tenant: Option<String>,
    pub fingerprint: String,
    pub not_after_unix: i64,
}

pub fn node_certificate(certificate: &CertificateDer<'_>) -> anyhow::Result<NodeCertificate> {
    let peer = inspect(certificate)?;
    if peer.other_names > 0 {
        bail!(
            "the node certificate carries {} names that are not URIs",
            peer.other_names
        );
    }
    let identity =
        nightfall_provisioning::identity::identity_from_uris(peer.uris.iter().map(String::as_str))?;
    Ok(NodeCertificate {
        device_id: identity.device_id,
        installation_id: identity.installation_id,
        tenant: identity.tenant,
        fingerprint: peer.fingerprint,
        not_after_unix: peer.not_after_unix,
    })
}

fn is_principal_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAXIMUM_PRINCIPAL_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'@'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrincipalCertificate {
    pub principal: String,
    pub fingerprint: String,
    pub fingerprint_bytes: [u8; 32],
    pub not_after_unix: i64,
}

pub fn principal_certificate(
    certificate: &CertificateDer<'_>,
) -> anyhow::Result<PrincipalCertificate> {
    let peer = inspect(certificate)?;
    if peer
        .uris
        .iter()
        .any(|uri| uri.starts_with(DEVICE_PREFIX) || uri.starts_with(INSTALLATION_PREFIX))
    {
        bail!("a principal certificate carries a node identity");
    }
    let principals: Vec<&str> = peer
        .uris
        .iter()
        .filter_map(|uri| uri.strip_prefix(PRINCIPAL_PREFIX))
        .collect();
    let [principal] = principals.as_slice() else {
        bail!(
            "a principal certificate must carry exactly one {PRINCIPAL_PREFIX} name, not {}",
            principals.len()
        );
    };
    if !is_principal_name(principal) {
        bail!("the principal name {principal:?} is not valid");
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, certificate.as_ref());
    let mut fingerprint_bytes = [0u8; 32];
    fingerprint_bytes.copy_from_slice(digest.as_ref());
    Ok(PrincipalCertificate {
        principal: principal.to_string(),
        fingerprint: peer.fingerprint,
        fingerprint_bytes,
        not_after_unix: peer.not_after_unix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::tests::Authority;

    #[test]
    fn reads_a_node_identity_from_its_san_uris() {
        let authority = Authority::new("fleet-client");
        let (certificate, _, _, _) = authority.issue(
            &[],
            &[
                "urn:dusk:device:00112233445566778899aabbccddeeff",
                "urn:dusk:installation:ffeeddccbbaa99887766554433221100",
                "urn:dusk:tenant:acme",
            ],
            true,
        );
        let node = node_certificate(&certificate).unwrap();
        assert_eq!(node.device_id, "00112233445566778899aabbccddeeff");
        assert_eq!(node.installation_id, "ffeeddccbbaa99887766554433221100");
        assert_eq!(node.tenant.as_deref(), Some("acme"));
        assert_eq!(node.fingerprint.len(), 64);
        assert!(node.not_after_unix > 0);
        let (missing, _, _, _) = authority.issue(
            &[],
            &["urn:dusk:device:00112233445566778899aabbccddeeff"],
            true,
        );
        assert!(node_certificate(&missing).is_err());
        let (named, _, _, _) = authority.issue(
            &["node.example"],
            &[
                "urn:dusk:device:00112233445566778899aabbccddeeff",
                "urn:dusk:installation:ffeeddccbbaa99887766554433221100",
            ],
            true,
        );
        assert!(node_certificate(&named).is_err());
    }

    #[test]
    fn requires_exactly_one_principal_and_no_node_identity() {
        let authority = Authority::new("internal");
        let (good, _, _, _) = authority.issue(&[], &["urn:dusk:principal:dawn-0"], true);
        let principal = principal_certificate(&good).unwrap();
        assert_eq!(principal.principal, "dawn-0");
        assert_eq!(principal.fingerprint, sha256_hex(good.as_ref()));
        let (none, _, _, _) = authority.issue(&["dawn.example"], &[], true);
        assert!(principal_certificate(&none).is_err());
        let (two, _, _, _) = authority.issue(
            &[],
            &["urn:dusk:principal:dawn-0", "urn:dusk:principal:dawn-1"],
            true,
        );
        assert!(principal_certificate(&two).is_err());
        let (node, _, _, _) = authority.issue(
            &[],
            &[
                "urn:dusk:principal:dawn-0",
                "urn:dusk:device:00112233445566778899aabbccddeeff",
            ],
            true,
        );
        assert!(principal_certificate(&node).is_err());
        let (bad, _, _, _) = authority.issue(&[], &["urn:dusk:principal:dawn%200"], true);
        assert!(principal_certificate(&bad).is_err());
    }
}
