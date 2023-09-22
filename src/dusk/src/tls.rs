use anyhow::Result;
use core::pin::Pin;
use embedded_io_async::{Read, Write};
use embedded_tls::{Aes256GcmSha384, NoVerify, TlsConfig, TlsConnection, TlsContext};
use futures::{AsyncRead, AsyncWrite};
use rand_chacha::{rand_core::SeedableRng, ChaChaRng};

struct AsyncReaderWriter {
    pub reader: Pin<Box<dyn AsyncRead>>,
    pub writer: Pin<Box<dyn AsyncWrite>>,
}

// impl Read for AsyncReaderWriter {}
//
// impl Write for AsyncReaderWriter {}

pub async fn wrap_with_tls(
    reader: Pin<Box<dyn AsyncRead>>,
    writer: Pin<Box<dyn AsyncWrite>>,
) -> (Pin<Box<dyn AsyncRead>>, Pin<Box<dyn AsyncWrite>>) {
    (reader, writer)
}
//     // Buffers need to be 16k to support full size tls frames
//     let mut read_record_buffer = [0; 16384];
//     let mut write_record_buffer = [0; 16384];
//
//     // TODO: Provide a real rng seed
//     // Note seed has to be 256 bit to be crypto secure
//     let mut rng = ChaChaRng::from_seed([0; 32]);
//
//     let config = TlsConfig::new();
//
//     let mut tls: TlsConnection<AsyncReaderWriter, Aes256GcmSha384> = TlsConnection::new(
//         AsyncReaderWriter { reader, writer },
//         &mut read_record_buffer,
//         &mut write_record_buffer,
//     );
//
//     tls.open::<ChaChaRng, NoVerify>(TlsContext::new(&config, &mut rng))
//         .await?;
//
//     tls
// }
