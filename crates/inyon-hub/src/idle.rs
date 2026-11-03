//! Connections that go quiet are closed, as Bun's `idleTimeout: 120` closes them: one that sends
//! and takes nothing for two minutes (sent nothing at all, stopped halfway through its headers or
//! its upload, or stopped reading what the hub writes) ends with an error, which hyper answers by
//! dropping it. axum's server sets no timer of its own, so without this a client could hold a
//! connection, and one of the hub's file descriptors, for as long as it liked.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

/// Bun's idleTimeout
pub const IDLE: Duration = Duration::from_secs(120);

/// A connection that fails once it has neither read nor written anything for `limit`.
pub struct Idle<T> {
    io: T,
    limit: Duration,
    deadline: Pin<Box<Sleep>>,
}

impl<T> Idle<T> {
    pub fn new(io: T, limit: Duration) -> Self {
        Idle { io, limit, deadline: Box::pin(tokio::time::sleep(limit)) }
    }

    fn moved(&mut self) {
        let next = Instant::now() + self.limit;
        self.deadline.as_mut().reset(next);
    }

    /// While the connection waits, so does the deadline; it running out is the error.
    fn waited<R>(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<R>> {
        match self.deadline.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "the connection was idle too long"))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Idle<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut me.io).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    me.moved();
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => me.waited(cx),
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Idle<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        match Pin::new(&mut me.io).poll_write(cx, data) {
            Poll::Ready(Ok(n)) => {
                if n > 0 {
                    me.moved();
                }
                Poll::Ready(Ok(n))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => me.waited(cx),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        match Pin::new(&mut me.io).poll_flush(cx) {
            Poll::Pending => me.waited(cx),
            done => done,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        match Pin::new(&mut me.io).poll_shutdown(cx) {
            Poll::Pending => me.waited(cx),
            done => done,
        }
    }
}

/// Any listener, its connections wrapped in [`Idle`].
pub struct IdleListener<L> {
    inner: L,
    limit: Duration,
}

impl<L> IdleListener<L> {
    pub fn new(inner: L, limit: Duration) -> Self {
        IdleListener { inner, limit }
    }
}

impl<L: axum::serve::Listener> axum::serve::Listener for IdleListener<L>
where
    L::Io: Unpin,
{
    type Io = Idle<L::Io>;
    type Addr = L::Addr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (io, addr) = self.inner.accept().await;
        (Idle::new(io, self.limit), addr)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_hub_gives_up_on_a_machines_temp_files_before_the_idle_limit_drops_the_app() {
        use inyon_core::features::temp;
        let idle = u64::try_from(super::IDLE.as_millis()).unwrap();
        assert!(temp::WAIT < temp::DELETE_WAIT && temp::DELETE_WAIT < idle);
    }

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn server(limit: Duration) -> std::net::SocketAddr {
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "hi" }));
        tokio::spawn(async move { axum::serve(IdleListener::new(tcp, limit), app).await });
        addr
    }

    /// what the server does with a connection within `within`: Some(bytes) once it closes it
    async fn closed_within(c: &mut tokio::net::TcpStream, within: Duration) -> Option<Vec<u8>> {
        let mut got = vec![];
        let mut buf = [0u8; 1024];
        let end = Instant::now() + within;
        loop {
            match tokio::time::timeout_at(end, c.read(&mut buf)).await {
                Ok(Ok(0)) | Ok(Err(_)) => return Some(got),
                Ok(Ok(n)) => got.extend_from_slice(&buf[..n]),
                Err(_) => return None,
            }
        }
    }

    #[test]
    fn a_connection_that_sends_nothing_or_half_its_headers_is_closed() {
        crate::test_util::block(async {
            let addr = server(Duration::from_millis(300)).await;
            let mut silent = tokio::net::TcpStream::connect(addr).await.unwrap();
            let mut half = tokio::net::TcpStream::connect(addr).await.unwrap();
            half.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n").await.unwrap();
            assert!(closed_within(&mut silent, Duration::from_secs(3)).await.is_some(), "a silent connection is closed");
            assert!(closed_within(&mut half, Duration::from_secs(3)).await.is_some(), "half a request is closed");
        });
    }

    #[test]
    fn a_connection_in_use_stays_and_an_idle_keep_alive_one_goes() {
        crate::test_util::block(async {
            let addr = server(Duration::from_millis(400)).await;
            let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
                let mut buf = [0u8; 512];
                let n = c.read(&mut buf).await.unwrap();
                assert!(String::from_utf8_lossy(&buf[..n]).ends_with("hi"));
            }
            assert!(closed_within(&mut c, Duration::from_secs(3)).await.is_some(), "kept alive, then idle: closed");
        });
    }
}
