use alloc::boxed::Box;
use rustls::crypto::hash::{Context, Hash, HashAlgorithm, Output};
use sha2::Digest as _;

pub(crate) struct Sha256;

struct Sha256Context(sha2::Sha256);

impl Hash for Sha256 {
    fn start(&self) -> Box<dyn Context> {
        Box::new(Sha256Context(sha2::Sha256::new()))
    }

    fn hash(&self, data: &[u8]) -> Output {
        Output::new(&sha2::Sha256::digest(data))
    }

    fn output_len(&self) -> usize {
        32
    }

    fn algorithm(&self) -> HashAlgorithm {
        HashAlgorithm::SHA256
    }
}

impl Context for Sha256Context {
    fn fork_finish(&self) -> Output {
        Output::new(&self.0.clone().finalize())
    }

    fn fork(&self) -> Box<dyn Context> {
        Box::new(Sha256Context(self.0.clone()))
    }

    fn finish(self: Box<Self>) -> Output {
        Output::new(&self.0.finalize())
    }

    fn update(&mut self, data: &[u8]) {
        self.0.update(data)
    }
}
