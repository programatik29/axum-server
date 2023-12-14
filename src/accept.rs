//! [`Accept`] trait and utilities.

use std::{
    future::{Future, Ready},
    io,
};

use tokio::net::TcpStream;

/// An asynchronous function to modify io stream and service.
pub trait Accept<I, S> {
    /// IO stream produced by accept.
    type Stream;

    /// Service produced by accept.
    type Service;

    /// Future return value.
    type Future: Future<Output = io::Result<(Self::Stream, Self::Service)>>;

    /// Process io stream and service asynchronously.
    fn accept(&self, stream: I, service: S) -> Self::Future;
}

/// A no-op acceptor.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultAcceptor;

impl DefaultAcceptor {
    /// Create a new default acceptor.
    pub fn new() -> Self {
        Self
    }
}

impl<I, S> Accept<I, S> for DefaultAcceptor {
    type Stream = I;
    type Service = S;
    type Future = Ready<io::Result<(Self::Stream, Self::Service)>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        std::future::ready(Ok((stream, service)))
    }
}

/// An acceptor that sets `TCP_NODELAY` on accepted streams.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDelayAcceptor;

impl NoDelayAcceptor {
    /// Create a new acceptor that sets `TCP_NODELAY` on accepted streams.
    pub fn new() -> Self {
        Self
    }
}

impl<S> Accept<TcpStream, S> for NoDelayAcceptor {
    type Stream = TcpStream;
    type Service = S;
    type Future = Ready<io::Result<(Self::Stream, Self::Service)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        std::future::ready(stream.set_nodelay(true).and(Ok((stream, service))))
    }
}
