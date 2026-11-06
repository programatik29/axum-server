//! [`Accept`] trait and utilities.

use http::Request;
use hyper::body::Incoming;
use socket2::SockRef;
use std::{
    fmt,
    future::{Future, Ready},
    io,
    net::SocketAddr,
    time::Duration,
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

/// TCP keepalive parameters applied by [`TcpConfigAcceptor`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(dead_code)]
pub struct TcpKeepaliveConfig {
    time: Option<Duration>,
    interval: Option<Duration>,
    retries: Option<u32>,
}

impl TcpKeepaliveConfig {
    /// Create a new keepalive configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the amount of time an idle connection must be idle before the
    /// first keepalive probe is sent.
    pub fn time(mut self, time: Duration) -> Self {
        self.time = Some(time);
        self
    }

    /// Set the time between keepalive probes.
    ///
    /// This is only supported on some platforms; it is ignored elsewhere.
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = Some(interval);
        self
    }

    /// Set the number of failed probes before the connection is closed.
    ///
    /// This is only supported on some platforms; it is ignored elsewhere.
    pub fn retries(mut self, retries: u32) -> Self {
        self.retries = Some(retries);
        self
    }
}

/// TCP socket options applied to accepted connections.
#[derive(Clone, Copy, Debug, Default)]
pub struct TcpSocketConfig {
    nodelay: bool,
    keepalive: Option<TcpKeepaliveConfig>,
}

impl TcpSocketConfig {
    /// Create a new configuration that leaves the socket options unchanged.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable `TCP_NODELAY`.
    pub fn nodelay(mut self, nodelay: bool) -> Self {
        self.nodelay = nodelay;
        self
    }

    /// Set the keepalive parameters. Keepalive is disabled by default.
    pub fn keepalive(mut self, keepalive: TcpKeepaliveConfig) -> Self {
        self.keepalive = Some(keepalive);
        self
    }
}

/// An acceptor that applies TCP socket options to accepted connections.
///
/// It can be composed with TLS acceptors by setting it as the inner acceptor:
/// `RustlsAcceptor::new(config).acceptor(TcpConfigAcceptor::new(...))`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TcpConfigAcceptor {
    config: TcpSocketConfig,
}

impl TcpConfigAcceptor {
    /// Create a new acceptor with the provided socket options.
    pub fn new(config: TcpSocketConfig) -> Self {
        Self { config }
    }
}

impl<S> Accept<TcpStream, S> for TcpConfigAcceptor {
    type Stream = TcpStream;
    type Service = S;
    type Future = Ready<io::Result<(Self::Stream, Self::Service)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        std::future::ready(
            (|| {
                if self.config.nodelay {
                    stream.set_nodelay(true)?;
                }

                if let Some(config) = self.config.keepalive {
                    let mut keepalive = socket2::TcpKeepalive::new();

                    if let Some(time) = config.time {
                        keepalive = keepalive.with_time(time);
                    }

                    #[cfg(any(
                        target_os = "android",
                        target_os = "dragonfly",
                        target_os = "freebsd",
                        target_os = "fuchsia",
                        target_os = "illumos",
                        target_os = "ios",
                        target_os = "visionos",
                        target_os = "linux",
                        target_os = "macos",
                        target_os = "netbsd",
                        target_os = "tvos",
                        target_os = "watchos",
                        target_os = "windows",
                        target_os = "cygwin",
                    ))]
                    if let Some(interval) = config.interval {
                        keepalive = keepalive.with_interval(interval);
                    }

                    #[cfg(any(
                        target_os = "android",
                        target_os = "dragonfly",
                        target_os = "freebsd",
                        target_os = "fuchsia",
                        target_os = "illumos",
                        target_os = "ios",
                        target_os = "visionos",
                        target_os = "linux",
                        target_os = "macos",
                        target_os = "netbsd",
                        target_os = "tvos",
                        target_os = "watchos",
                        target_os = "windows",
                        target_os = "cygwin",
                    ))]
                    if let Some(retries) = config.retries {
                        keepalive = keepalive.with_retries(retries);
                    }

                    SockRef::from(&stream).set_tcp_keepalive(&keepalive)?;
                }

                Ok(())
            })()
            .map(|()| (stream, service)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Accept, TcpConfigAcceptor, TcpKeepaliveConfig, TcpSocketConfig};
    use std::time::Duration;
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn applies_nodelay_and_keepalive() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (client, server) = tokio::join!(TcpStream::connect(addr), listener.accept());
        let _client = client.unwrap();
        let (server, _) = server.unwrap();

        let acceptor = TcpConfigAcceptor::new(
            TcpSocketConfig::new()
                .nodelay(true)
                .keepalive(TcpKeepaliveConfig::new().time(Duration::from_secs(30))),
        );

        let (stream, ()) = acceptor.accept(server, ()).await.unwrap();

        assert!(stream.nodelay().unwrap());

        #[cfg(target_os = "linux")]
        {
            use socket2::SockRef;

            let socket = SockRef::from(&stream);
            assert_eq!(
                socket.tcp_keepalive_time().unwrap(),
                Duration::from_secs(30)
            );
        }
    }
}
