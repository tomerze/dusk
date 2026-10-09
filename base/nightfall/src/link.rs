use std::prelude::rust_2024::*;

use std::cell::{Cell, RefCell};
use std::io::{self, ErrorKind};
use std::net::{Shutdown, TcpStream};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use dusk_program::embassy_sync::blocking_mutex::raw::NoopRawMutex;
use dusk_program::embassy_sync::signal::Signal;
use dusk_program::embassy_time::Instant;
use futures::{AsyncRead, AsyncReadExt as _, AsyncWrite};

pub(crate) struct LinkState {
    last_read: Cell<Instant>,
    closed: Cell<bool>,
    read_waker: RefCell<Option<Waker>>,
    write_waker: RefCell<Option<Waker>>,
    released: Signal<NoopRawMutex, ()>,
    socket: Option<TcpStream>,
}

impl LinkState {
    pub(crate) fn new(socket: Option<TcpStream>) -> Self {
        LinkState {
            last_read: Cell::new(Instant::now()),
            closed: Cell::new(false),
            read_waker: RefCell::new(None),
            write_waker: RefCell::new(None),
            released: Signal::new(),
            socket,
        }
    }

    pub(crate) fn last_read(&self) -> Instant {
        self.last_read.get()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.get()
    }

    pub(crate) fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        if let Some(socket) = &self.socket
            && let Err(error) = socket.shutdown(Shutdown::Both)
            && error.kind() != ErrorKind::NotConnected
        {
            tracing::warn!(error = %error, "couldn't shut down the fleet link's socket");
        }
        for waker in [&self.read_waker, &self.write_waker] {
            if let Some(waker) = waker.borrow_mut().take() {
                waker.wake();
            }
        }
    }

    pub(crate) async fn released(&self) {
        self.released.wait().await;
        self.released.signal(());
    }
}

pub(crate) struct Watched<Stream> {
    inner: Stream,
    state: Rc<LinkState>,
}

impl<Stream> Watched<Stream> {
    pub(crate) fn new(inner: Stream, state: Rc<LinkState>) -> Self {
        Watched { inner, state }
    }
}

impl<Stream> Drop for Watched<Stream> {
    fn drop(&mut self) {
        self.state.released.signal(());
    }
}

fn closed_error() -> io::Error {
    io::Error::new(ErrorKind::ConnectionAborted, "the fleet link was closed")
}

impl<Stream: AsyncRead + Unpin> AsyncRead for Watched<Stream> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.state.is_closed() {
            return Poll::Ready(Err(closed_error()));
        }
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_read(context, buffer) {
            Poll::Ready(Ok(count)) => {
                if count > 0 {
                    this.state.last_read.set(Instant::now());
                }
                Poll::Ready(Ok(count))
            }
            Poll::Pending => {
                *this.state.read_waker.borrow_mut() = Some(context.waker().clone());
                Poll::Pending
            }
            ready => ready,
        }
    }
}

impl<Stream: AsyncWrite + Unpin> AsyncWrite for Watched<Stream> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.state.is_closed() {
            return Poll::Ready(Err(closed_error()));
        }
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_write(context, buffer);
        if polled.is_pending() {
            *this.state.write_waker.borrow_mut() = Some(context.waker().clone());
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.state.is_closed() {
            return Poll::Ready(Err(closed_error()));
        }
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_flush(context);
        if polled.is_pending() {
            *this.state.write_waker.borrow_mut() = Some(context.waker().clone());
        }
        polled
    }

    fn poll_close(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.state.is_closed() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_close(context)
    }
}

pub(crate) struct LinkGuard(pub Rc<LinkState>);

impl Drop for LinkGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

pub(crate) type Halves<Stream> = (
    futures::io::ReadHalf<Watched<Stream>>,
    futures::io::WriteHalf<Watched<Stream>>,
);

pub(crate) fn watch<Stream: AsyncRead + AsyncWrite + Unpin>(
    stream: Stream,
    socket: Option<TcpStream>,
) -> (Rc<LinkState>, Halves<Stream>) {
    let state = Rc::new(LinkState::new(socket));
    let halves = Watched::new(stream, state.clone()).split();
    (state, halves)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::AsyncWriteExt as _;
    use futures::executor::block_on;

    #[test]
    fn reads_move_the_last_read_instant() {
        let (state, (mut reader, _writer)) =
            watch(futures::io::Cursor::new(b"heartbeat".to_vec()), None);
        let before = state.last_read();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut buffer = [0u8; 4];
        assert_eq!(block_on(reader.read(&mut buffer)).unwrap(), 4);
        assert!(state.last_read() > before);
    }

    #[test]
    fn an_empty_read_does_not_count_as_traffic() {
        let (state, (mut reader, _writer)) = watch(futures::io::Cursor::new(Vec::new()), None);
        let before = state.last_read();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut buffer = [0u8; 4];
        assert_eq!(block_on(reader.read(&mut buffer)).unwrap(), 0);
        assert_eq!(state.last_read(), before);
    }

    #[test]
    fn closing_fails_every_later_read_and_write() {
        let (state, (mut reader, mut writer)) =
            watch(futures::io::Cursor::new(b"data".to_vec()), None);
        state.close();
        state.close();
        let mut buffer = [0u8; 4];
        assert_eq!(
            block_on(reader.read(&mut buffer)).unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
        assert_eq!(
            block_on(writer.write(b"more")).unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
    }

    #[test]
    fn closing_shuts_the_socket_down() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let state = LinkState::new(Some(client.try_clone().unwrap()));
        state.close();
        let mut buffer = [0u8; 1];
        assert_eq!(std::io::Read::read(&mut server, &mut buffer).unwrap(), 0);
    }

    #[test]
    fn released_fires_once_both_halves_are_gone() {
        let (state, (reader, writer)) = watch(futures::io::Cursor::new(Vec::new()), None);
        drop(reader);
        let early = futures::FutureExt::now_or_never(state.released());
        assert!(early.is_none());
        drop(writer);
        block_on(state.released());
        block_on(state.released());
    }

    #[test]
    fn the_guard_closes_the_link() {
        let (state, _halves) = watch(futures::io::Cursor::new(Vec::new()), None);
        drop(LinkGuard(state.clone()));
        assert!(state.is_closed());
    }
}
