//! A request's or an answer's body: nothing, bytes in hand, or a stream of them (a file being sent,
//! a request still arriving). Sendable, so the hub can hand an answer from its core thread to the
//! connection's task.

use std::io;

use bytes::Bytes;
use futures_util::stream::{BoxStream, Stream, StreamExt};

pub enum Body {
    Empty,
    Full(Bytes),
    Stream(BoxStream<'static, io::Result<Bytes>>),
}

impl Body {
    pub fn empty() -> Self {
        Body::Empty
    }

    pub fn from_stream(s: impl Stream<Item = io::Result<Bytes>> + Send + 'static) -> Self {
        Body::Stream(s.boxed())
    }

    /// The next piece, or None at the end.
    pub async fn next_chunk(&mut self) -> Option<io::Result<Bytes>> {
        match self {
            Body::Empty => None,
            Body::Full(b) => {
                let b = std::mem::take(b);
                *self = Body::Empty;
                (!b.is_empty()).then_some(Ok(b))
            }
            Body::Stream(s) => s.next().await,
        }
    }

    /// All of it.
    pub async fn bytes(mut self) -> io::Result<Bytes> {
        if let Body::Full(b) = self {
            return Ok(b);
        }
        let mut out = Vec::new();
        while let Some(c) = self.next_chunk().await {
            out.extend_from_slice(&c?);
        }
        Ok(Bytes::from(out))
    }
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Body::Empty => f.write_str("Body::Empty"),
            Body::Full(b) => write!(f, "Body::Full({} bytes)", b.len()),
            Body::Stream(_) => f.write_str("Body::Stream"),
        }
    }
}

impl From<Bytes> for Body {
    fn from(b: Bytes) -> Self {
        Body::Full(b)
    }
}

impl From<Vec<u8>> for Body {
    fn from(b: Vec<u8>) -> Self {
        Body::Full(Bytes::from(b))
    }
}

impl From<String> for Body {
    fn from(s: String) -> Self {
        Body::Full(Bytes::from(s))
    }
}

impl From<&'static str> for Body {
    fn from(s: &'static str) -> Self {
        Body::Full(Bytes::from_static(s.as_bytes()))
    }
}

impl From<&'static [u8]> for Body {
    fn from(s: &'static [u8]) -> Self {
        Body::Full(Bytes::from_static(s))
    }
}
