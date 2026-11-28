use core::fmt;
use core::marker::PhantomData;
use rustls::crypto::{GetRandomFailed, SecureRandom};

pub trait Entropy: Send + Sync + 'static {
    fn fill(destination: &mut [u8]) -> Result<(), GetRandomFailed>;
}

pub(crate) struct Random<E: Entropy>(PhantomData<E>);

impl<E: Entropy> Random<E> {
    pub(crate) const INSTANCE: Self = Random(PhantomData);
}

impl<E: Entropy> fmt::Debug for Random<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Random")
    }
}

impl<E: Entropy> SecureRandom for Random<E> {
    fn fill(&self, destination: &mut [u8]) -> Result<(), GetRandomFailed> {
        E::fill(destination)
    }
}
