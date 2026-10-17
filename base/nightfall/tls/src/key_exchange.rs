use crate::entropy::Entropy;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;
use p256::elliptic_curve::sec1::ToEncodedPoint as _;
use rustls::crypto::{ActiveKeyExchange, SharedSecret, SupportedKxGroup};
use rustls::{Error, NamedGroup, PeerMisbehaved};
use zeroize::Zeroize as _;

const DRAWS: usize = 8;

pub(crate) struct Secp256r1<E: Entropy>(PhantomData<E>);

impl<E: Entropy> Secp256r1<E> {
    pub(crate) const GROUP: Self = Secp256r1(PhantomData);
}

impl<E: Entropy> fmt::Debug for Secp256r1<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secp256r1")
    }
}

impl<E: Entropy> SupportedKxGroup for Secp256r1<E> {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let secret = (0..DRAWS)
            .find_map(|_| {
                let mut bytes = p256::FieldBytes::default();
                if E::fill(&mut bytes).is_err() {
                    return Some(Err(Error::FailedToGetRandomBytes));
                }
                let secret = p256::SecretKey::from_bytes(&bytes).ok();
                bytes.zeroize();
                secret.map(Ok)
            })
            .unwrap_or(Err(Error::FailedToGetRandomBytes))?;
        let public = secret
            .public_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec();
        Ok(Box::new(Secp256r1Exchange { secret, public }))
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}

struct Secp256r1Exchange {
    secret: p256::SecretKey,
    public: Vec<u8>,
}

impl ActiveKeyExchange for Secp256r1Exchange {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        let peer = p256::PublicKey::from_sec1_bytes(peer)
            .map_err(|_| Error::from(PeerMisbehaved::InvalidKeyShare))?;
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        Ok(SharedSecret::from(&shared.raw_secret_bytes()[..]))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}
