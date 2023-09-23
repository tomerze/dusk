use alloc::vec::Vec;
use core::pin::Pin;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::zerocopy_channel::{Channel, Receiver, Sender};
use futures::{AsyncRead, AsyncWrite};
use wolfssl::IOCallbackResult;

const CHANNEL_MESSAGE_COUNT: usize = 1;

struct IOChannels<'p> {
    receiver: Receiver<'p, NoopRawMutex, Vec<u8>>,
    sender: Sender<'p, NoopRawMutex, Vec<u8>>,
}

impl<'p> IOChannels<'p> {
    fn recv(&mut self, buf: &mut [u8]) -> IOCallbackResult<usize> {
        match self.receiver.try_receive() {
            Some(received_bytes) => {
                buf.copy_from_slice(received_bytes);
                IOCallbackResult::Ok(received_bytes.len())
            }
            None => IOCallbackResult::WouldBlock,
        }
    }

    fn send(&mut self, buf: &[u8]) -> IOCallbackResult<usize> {
        let send_bytes = match self.sender.try_send() {
            Some(send_bytes) => send_bytes,
            None => {
                return IOCallbackResult::WouldBlock;
            }
        };
        send_bytes.copy_from_slice(buf);
        self.sender.send_done();
        IOCallbackResult::Ok(send_bytes.len())
    }
}

pub async fn wrap_with_tls(
    reader: Pin<Box<dyn AsyncRead>>,
    writer: Pin<Box<dyn AsyncWrite>>,
) -> (Pin<Box<dyn AsyncRead>>, Pin<Box<dyn AsyncWrite>>) {
    let mut reader_messages = [[0; CHANNEL_MESSAGE_SIZE]; CHANNEL_MESSAGE_COUNT];
    let mut writer_messages = [[0; CHANNEL_MESSAGE_SIZE]; CHANNEL_MESSAGE_COUNT];
    let mut reader_channel =
        Channel::<NoopRawMutex, [u8; CHANNEL_MESSAGE_SIZE]>::new(&mut reader_messages);
    let mut writer_channel =
        Channel::<NoopRawMutex, [u8; CHANNEL_MESSAGE_SIZE]>::new(&mut writer_messages);

    let (reader_channel_sender, reader_channel_receiver) = reader_channel.split();
    let (writer_channel_sender, writer_channel_receiver) = writer_channel.split();

    let io_channels = IOChannels {
        receiver: reader_channel_receiver,
        sender: writer_channel_sender,
    };

    (reader, writer)
}
