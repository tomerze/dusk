use alloc::boxed::Box;
use capnp::{Error, ErrorKind, Result};
use core::future::poll_fn;
use core::pin::Pin;
use core::task::{Context, Poll};

pub trait AsyncRead {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<Result<usize>>;
}

pub trait AsyncWrite {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize>>;

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>>;

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>>;
}

impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for &mut T {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut **self).poll_read(context, buffer)
    }
}

impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for Box<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut **self).poll_read(context, buffer)
    }
}

impl<T: AsyncRead + ?Sized> AsyncRead for Pin<Box<T>> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<Result<usize>> {
        self.get_mut().as_mut().poll_read(context, buffer)
    }
}

impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for &mut T {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut **self).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut **self).poll_flush(context)
    }

    fn poll_close(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut **self).poll_close(context)
    }
}

impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for Box<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut **self).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut **self).poll_flush(context)
    }

    fn poll_close(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut **self).poll_close(context)
    }
}

impl<T: AsyncWrite + ?Sized> AsyncWrite for Pin<Box<T>> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize>> {
        self.get_mut().as_mut().poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        self.get_mut().as_mut().poll_flush(context)
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        self.get_mut().as_mut().poll_close(context)
    }
}

#[cfg(feature = "std")]
pub struct FromFutures<T>(pub T);

#[cfg(feature = "std")]
impl<T: futures_io::AsyncRead + Unpin> AsyncRead for FromFutures<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.0)
            .poll_read(context, buffer)
            .map_err(Error::from)
    }
}

#[cfg(feature = "std")]
impl<T: futures_io::AsyncWrite + Unpin> AsyncWrite for FromFutures<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.0)
            .poll_write(context, buffer)
            .map_err(Error::from)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.0)
            .poll_flush(context)
            .map_err(Error::from)
    }

    fn poll_close(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.0)
            .poll_close(context)
            .map_err(Error::from)
    }
}

pub async fn read<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
    buffer: &mut [u8],
) -> Result<usize> {
    poll_fn(|context| Pin::new(&mut *reader).poll_read(context, buffer)).await
}

pub async fn read_exact<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
    mut buffer: &mut [u8],
) -> Result<()> {
    while !buffer.is_empty() {
        let count = read(reader, buffer).await?;
        if count == 0 {
            return Err(Error::from_kind(ErrorKind::PrematureEndOfFile));
        }
        buffer = &mut buffer[count..];
    }
    Ok(())
}

pub async fn write_all<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    mut buffer: &[u8],
) -> Result<()> {
    while !buffer.is_empty() {
        let count = poll_fn(|context| Pin::new(&mut *writer).poll_write(context, buffer)).await?;
        if count == 0 {
            return Err(Error::failed("failed to write whole buffer".into()));
        }
        buffer = &buffer[count..];
    }
    Ok(())
}

pub async fn flush<W: AsyncWrite + Unpin + ?Sized>(writer: &mut W) -> Result<()> {
    poll_fn(|context| Pin::new(&mut *writer).poll_flush(context)).await
}

pub async fn close<W: AsyncWrite + Unpin + ?Sized>(writer: &mut W) -> Result<()> {
    poll_fn(|context| Pin::new(&mut *writer).poll_close(context)).await
}
