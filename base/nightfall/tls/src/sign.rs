use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use p256::ecdsa::signature::Signer as _;
use p256::pkcs8::DecodePrivateKey as _;
use rustls::crypto::KeyProvider;
use rustls::pki_types::PrivateKeyDer;
use rustls::sign::{Signer, SigningKey};
use rustls::{Error, SignatureAlgorithm, SignatureScheme};

#[derive(Debug)]
pub(crate) struct Keys;

impl KeyProvider for Keys {
    fn load_private_key(&self, key: PrivateKeyDer<'static>) -> Result<Arc<dyn SigningKey>, Error> {
        let PrivateKeyDer::Pkcs8(encoded) = key else {
            return Err(Error::General(String::from("the key is not in PKCS#8")));
        };
        let key = p256::ecdsa::SigningKey::from_pkcs8_der(encoded.secret_pkcs8_der()).map_err(
            |error| Error::General(format!("the key is not a PKCS#8 P-256 key: {error}")),
        )?;
        Ok(Arc::new(EcdsaP256Key(key)))
    }
}

#[derive(Debug)]
struct EcdsaP256Key(p256::ecdsa::SigningKey);

impl SigningKey for EcdsaP256Key {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        offered
            .contains(&SignatureScheme::ECDSA_NISTP256_SHA256)
            .then(|| Box::new(EcdsaP256Signer(self.0.clone())) as Box<dyn Signer>)
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        SignatureAlgorithm::ECDSA
    }
}

#[derive(Debug)]
struct EcdsaP256Signer(p256::ecdsa::SigningKey);

impl Signer for EcdsaP256Signer {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let signature: p256::ecdsa::Signature = self.0.sign(message);
        Ok(signature.to_der().as_bytes().to_vec())
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::ECDSA_NISTP256_SHA256
    }
}
