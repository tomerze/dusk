use core::pin::Pin;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::pipe::{Pipe, Reader, TryReadError, TryWriteError, Writer};
use futures::{AsyncRead, AsyncWrite};
use wolfssl::IOCallbackResult;
/*
* The plan is as follows:
* Each time wolfssl asks for bytes first check the pipe, if the pipe has bytes use all that you can.
* If you can fill the buffer with more bytes use `now_or_never` on the reader future
* directly.
*
* If and only if you have no bytes at all for wolfssl, return WouldBlock which would in turn
* trigger wolfssl to return PendingWouldBlock which would make us `await` for further bytes and put
* them in the pipe.
*
* This essentially means that when there is continues traffic we become zero-copy (apart from the
* wolfssl code), but when there isn't continues traffic we power-efficiently await for bytes.
*/

const PIPE_BUFFER_SIZE: usize = 2048;

struct WolfsslCallbacks<'a, 'p> {
    reader: &'a mut Pin<Box<dyn AsyncRead>>,
    writer: &'a mut Pin<Box<dyn AsyncWrite>>,
    reader_pipe_reader: Reader<'p, NoopRawMutex, PIPE_BUFFER_SIZE>,
    writer_pipe_writer: Writer<'p, NoopRawMutex, PIPE_BUFFER_SIZE>,
}

impl<'a, 'p> WolfsslCallbacks<'a, 'p> {
    fn recv(&mut self, buf: &mut [u8]) -> IOCallbackResult<usize> {
        match self.reader_pipe_reader.try_read(buf) {
            Ok(read_bytes) => IOCallbackResult::Ok(read_bytes),
            Err(TryReadError::Empty) => IOCallbackResult::WouldBlock,
        }
    }

    fn send(&mut self, buf: &[u8]) -> IOCallbackResult<usize> {
        match self.writer_pipe_writer.try_write(buf) {
            Ok(sent_bytes) => IOCallbackResult::Ok(sent_bytes),
            Err(TryWriteError::Full) => IOCallbackResult::WouldBlock,
        }
    }
}

pub async fn wrap_with_tls(
    mut reader: Pin<Box<dyn AsyncRead>>,
    mut writer: Pin<Box<dyn AsyncWrite>>,
) -> (Pin<Box<dyn AsyncRead>>, Pin<Box<dyn AsyncWrite>>) {
    let mut reader_pipe = Pipe::<NoopRawMutex, PIPE_BUFFER_SIZE>::new();
    let mut writer_pipe = Pipe::<NoopRawMutex, PIPE_BUFFER_SIZE>::new();

    let (reader_pipe_reader, reader_pipe_writer) = reader_pipe.split();
    let (writer_pipe_reader, writer_pipe_writer) = writer_pipe.split();

    let wolfssl_callbacks = WolfsslCallbacks {
        reader: &mut reader,
        writer: &mut writer,
        reader_pipe_reader,
        writer_pipe_writer,
    };

    (reader, writer)
}
