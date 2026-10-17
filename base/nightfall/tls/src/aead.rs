use alloc::boxed::Box;
use chacha20poly1305::ChaCha20Poly1305;
use chacha20poly1305::aead::{AeadInOut, KeyInit};
use rustls::crypto::cipher::{
    AeadKey, InboundOpaqueMessage, InboundPlainMessage, Iv, MessageDecrypter, MessageEncrypter,
    Nonce, OutboundOpaqueMessage, OutboundPlainMessage, PrefixedPayload, Tls13AeadAlgorithm,
    UnsupportedOperationError, make_tls13_aad,
};
use rustls::{ConnectionTrafficSecrets, ContentType, Error, ProtocolVersion};

const TAG_LENGTH: usize = 16;

pub(crate) struct Chacha20Poly1305;

struct Chacha20Poly1305Keys {
    cipher: ChaCha20Poly1305,
    initialization_vector: Iv,
}

impl Chacha20Poly1305Keys {
    fn new(key: AeadKey, initialization_vector: Iv) -> Self {
        Chacha20Poly1305Keys {
            cipher: ChaCha20Poly1305::new_from_slice(key.as_ref())
                .expect("rustls hands a ChaCha20-Poly1305 suite a 32-byte key"),
            initialization_vector,
        }
    }
}

impl Tls13AeadAlgorithm for Chacha20Poly1305 {
    fn encrypter(&self, key: AeadKey, initialization_vector: Iv) -> Box<dyn MessageEncrypter> {
        Box::new(Chacha20Poly1305Keys::new(key, initialization_vector))
    }

    fn decrypter(&self, key: AeadKey, initialization_vector: Iv) -> Box<dyn MessageDecrypter> {
        Box::new(Chacha20Poly1305Keys::new(key, initialization_vector))
    }

    fn key_len(&self) -> usize {
        32
    }

    fn extract_keys(
        &self,
        _key: AeadKey,
        _initialization_vector: Iv,
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Err(UnsupportedOperationError)
    }
}

impl MessageEncrypter for Chacha20Poly1305Keys {
    fn encrypt(
        &mut self,
        message: OutboundPlainMessage<'_>,
        sequence: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total_length = self.encrypted_payload_len(message.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total_length);
        payload.extend_from_chunks(&message.payload);
        payload.extend_from_slice(&message.typ.to_array());
        let nonce = Nonce::new(&self.initialization_vector, sequence).0;
        let additional_data = make_tls13_aad(total_length);
        let plain_length = payload.as_ref().len();
        let tag = self
            .cipher
            .encrypt_inout_detached(
                &nonce.into(),
                &additional_data,
                (&mut payload.as_mut()[..plain_length]).into(),
            )
            .map_err(|_| Error::EncryptError)?;
        payload.extend_from_slice(tag.as_slice());
        Ok(OutboundOpaqueMessage::new(
            ContentType::ApplicationData,
            ProtocolVersion::TLSv1_2,
            payload,
        ))
    }

    fn encrypted_payload_len(&self, payload_length: usize) -> usize {
        payload_length + 1 + TAG_LENGTH
    }
}

impl MessageDecrypter for Chacha20Poly1305Keys {
    fn decrypt<'message>(
        &mut self,
        mut message: InboundOpaqueMessage<'message>,
        sequence: u64,
    ) -> Result<InboundPlainMessage<'message>, Error> {
        let payload = &mut message.payload;
        let length = payload.len();
        if length < TAG_LENGTH {
            return Err(Error::DecryptError);
        }
        let nonce = Nonce::new(&self.initialization_vector, sequence).0;
        let additional_data = make_tls13_aad(length);
        let (text, tag) = payload.split_at_mut(length - TAG_LENGTH);
        let tag = chacha20poly1305::Tag::try_from(&*tag).map_err(|_| Error::DecryptError)?;
        self.cipher
            .decrypt_inout_detached(&nonce.into(), &additional_data, text.into(), &tag)
            .map_err(|_| Error::DecryptError)?;
        payload.truncate(length - TAG_LENGTH);
        message.into_tls13_unpadded_message()
    }
}
