use crate::aead::Chacha20Poly1305;
use crate::entropy::{Entropy, Random};
use crate::hash::Sha256;
use crate::hmac::HmacSha256;
use crate::key_exchange::Secp256r1;
use crate::sign::Keys;
use crate::verify::EcdsaP256Sha256;
use alloc::vec;
use rustls::crypto::tls13::HkdfUsingHmac;
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::{
    CipherSuite, CipherSuiteCommon, SignatureScheme, SupportedCipherSuite, Tls13CipherSuite,
};

static TLS13_CHACHA20_POLY1305_SHA256: Tls13CipherSuite = Tls13CipherSuite {
    common: CipherSuiteCommon {
        suite: CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
        hash_provider: &Sha256,
        confidentiality_limit: u64::MAX,
    },
    hkdf_provider: &HkdfUsingHmac(&HmacSha256),
    aead_alg: &Chacha20Poly1305,
    quic: None,
};

static ECDSA_P256_SHA256: EcdsaP256Sha256 = EcdsaP256Sha256;

static SIGNATURE_VERIFICATION: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[&ECDSA_P256_SHA256],
    mapping: &[(
        SignatureScheme::ECDSA_NISTP256_SHA256,
        &[&ECDSA_P256_SHA256],
    )],
};

pub fn provider<E: Entropy>() -> CryptoProvider {
    CryptoProvider {
        cipher_suites: vec![SupportedCipherSuite::Tls13(&TLS13_CHACHA20_POLY1305_SHA256)],
        kx_groups: vec![&Secp256r1::<E>::GROUP],
        signature_verification_algorithms: SIGNATURE_VERIFICATION,
        secure_random: &Random::<E>::INSTANCE,
        key_provider: &Keys,
    }
}
