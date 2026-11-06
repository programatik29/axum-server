//! [`Accept`] trait and utilities.

use http::Request;
use hyper::body::Incoming;
use std::{
    fmt,
    future::{Future, Ready},
    io,
    net::SocketAddr,
};
use tokio::net::TcpStream;
use tower_service::Service;

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

/// The local address of an accepted connection.
///
/// [`LocalAddrAcceptor`] adds this to the extensions of every request on the
/// connection. With `axum`, retrieve it with `Extension<LocalAddr>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalAddr(pub SocketAddr);

/// An acceptor that adds the local address of the connection to request
/// extensions.
///
/// This is useful when the server listens on `0.0.0.0` or another address with
/// multiple interfaces and the handler needs to know which local address the
/// client connected to.
///
/// It can be composed with TLS acceptors by setting it as the inner acceptor:
/// `RustlsAcceptor::new(config).acceptor(LocalAddrAcceptor::new())`.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalAddrAcceptor;

impl LocalAddrAcceptor {
    /// Create a new acceptor that adds the local address to request extensions.
    pub fn new() -> Self {
        Self
    }
}

impl<S> Accept<TcpStream, S> for LocalAddrAcceptor {
    type Stream = TcpStream;
    type Service = LocalAddrService<S>;
    type Future = Ready<io::Result<(Self::Stream, Self::Service)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        std::future::ready(stream.local_addr().map(|local_addr| {
            let service = LocalAddrService {
                inner: service,
                local_addr: LocalAddr(local_addr),
            };

            (stream, service)
        }))
    }
}

/// A service that adds [`LocalAddr`] to request extensions.
#[derive(Clone)]
pub struct LocalAddrService<S> {
    inner: S,
    local_addr: LocalAddr,
}

impl<S> fmt::Debug for LocalAddrService<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalAddrService")
            .field("local_addr", &self.local_addr)
            .finish_non_exhaustive()
    }
}

impl<S> Service<Request<Incoming>> for LocalAddrService<S>
where
    S: Service<Request<Incoming>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Incoming>) -> Self::Future {
        req.extensions_mut().insert(self.local_addr);

        self.inner.call(req)
    }
}
