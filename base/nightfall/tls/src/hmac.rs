use ::hmac::Mac as _;
use alloc::boxed::Box;
use rustls::crypto::hmac::{Hmac, Key, Tag};

pub(crate) struct HmacSha256;

struct HmacSha256Key(::hmac::Hmac<sha2::Sha256>);

impl Hmac for HmacSha256 {
    fn with_key(&self, key: &[u8]) -> Box<dyn Key> {
        Box::new(HmacSha256Key(
            ::hmac::Hmac::<sha2::Sha256>::new_from_slice(key)
                .expect("HMAC takes a key of any length"),
        ))
    }

    fn hash_output_len(&self) -> usize {
        32
    }
}

impl Key for HmacSha256Key {
    fn sign_concat(&self, first: &[u8], middle: &[&[u8]], last: &[u8]) -> Tag {
        let mut authenticator = self.0.clone();
        authenticator.update(first);
        for part in middle {
            authenticator.update(part);
        }
        authenticator.update(last);
        Tag::new(&authenticator.finalize().into_bytes())
    }

    fn tag_len(&self) -> usize {
        32
    }
}
