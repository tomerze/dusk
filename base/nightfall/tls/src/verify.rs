use p256::ecdsa::signature::Verifier as _;
use rustls::pki_types::{
    AlgorithmIdentifier, InvalidSignature, SignatureVerificationAlgorithm, alg_id,
};

#[derive(Debug)]
pub(crate) struct EcdsaP256Sha256;

impl SignatureVerificationAlgorithm for EcdsaP256Sha256 {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        let key =
            p256::ecdsa::VerifyingKey::from_sec1_bytes(public_key).map_err(|_| InvalidSignature)?;
        let signature =
            p256::ecdsa::DerSignature::try_from(signature).map_err(|_| InvalidSignature)?;
        key.verify(message, &signature)
            .map_err(|_| InvalidSignature)
    }

    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::ECDSA_P256
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::ECDSA_SHA256
    }
}
