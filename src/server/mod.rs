mod serve;

#[cfg(feature = "tls-rustls")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub mod tls;

#[cfg(feature = "record")]
#[cfg_attr(docsrs, doc(cfg(feature = "record")))]
pub mod record;

use serve::{Accept, HttpServer, Serve};

#[cfg(feature = "tls-rustls")]
use tls::TlsServer;

#[cfg(feature = "record")]
use record::RecordingHttpServer;

#[cfg(feature = "tls-rustls")]
use std::path::Path;

use std::io;
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs};

use futures_util::future::Ready;
use futures_util::stream::{FuturesUnordered, StreamExt};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::{spawn_blocking, JoinHandle};

use tower_service::Service;

use http::request::Request;
use http::response::Response;
use http_body::Body;

pub(crate) type BoxedToSocketAddrs =
    Box<dyn ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send>;

/// Configurable HTTP server, supporting HTTP/1.1 and HTTP2.
///
/// `Server` can conveniently be turned into a [`TlsServer`](TlsServer) with related methods.
///
/// See [main](crate) page for HTTP example. See [`axum_server::tls`](tls) module for HTTPS example.
#[derive(Default)]
pub struct Server {
    addrs: Vec<BoxedToSocketAddrs>,
}

impl Server {
    /// Create a new `Server`.
    ///
    /// Must bind to an address before calling [`serve`](Server::serve).
    pub fn new() -> Self {
        Server::default()
    }

    /// Bind to a single address or multiple addresses.
    pub fn bind<A>(mut self, addr: A) -> Self
    where
        A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
    {
        self.addrs.push(Box::new(addr));
        self
    }

    /// Bind to a single address or multiple addresses. Using tls protocol on streams.
    ///
    /// Certificate and private key must be set before or after calling this.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn bind_rustls<A>(self, addr: A) -> TlsServer
    where
        A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
    {
        TlsServer::from(self).bind_rustls(addr)
    }

    /// Load private key in PEM format.
    ///
    /// Successive calls will overwrite latest private key.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn private_key(self, key: Vec<u8>) -> TlsServer {
        TlsServer::from(self).private_key(key)
    }

    /// Load certificate(s) in PEM format.
    ///
    /// Successive calls will overwrite latest certificate.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn certificate(self, cert: Vec<u8>) -> TlsServer {
        TlsServer::from(self).certificate(cert)
    }

    /// Load private key from file in PEM format.
    ///
    /// Successive calls will overwrite latest private key.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn private_key_file(self, path: impl AsRef<Path>) -> TlsServer {
        TlsServer::from(self).private_key_file(path)
    }

    /// Load certificate(s) from file in PEM format.
    ///
    /// Successive calls will overwrite latest certificate.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn certificate_file(self, path: impl AsRef<Path>) -> TlsServer {
        TlsServer::from(self).certificate_file(path)
    }

    /// Serve provided cloneable service on all binded addresses.
    ///
    /// If accepting connection fails in any one of binded addresses, listening in all
    /// binded addresses will be stopped and then an error will be returned.
    pub async fn serve<S, B>(self, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Sync + Clone + 'static,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.custom_serve(move || HttpServer::new(NoopAcceptor::new(), service.clone()))
            .await
    }

    /// Serve provided cloneable service on all binded addresses.
    ///
    /// Record sent and received bytes for each connection. Sent and received bytes
    /// through a connection can be accessed through [`Request`](Request) extensions.
    ///
    /// See [`axum_server::record`](record) module for examples.
    ///
    /// If accepting connection fails in any one of binded addresses, listening in all
    /// binded addresses will be stopped and then an error will be returned.
    #[cfg(feature = "record")]
    #[cfg_attr(docsrs, doc(cfg(feature = "record")))]
    pub async fn serve_and_record<S, B>(self, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Sync + Clone + 'static,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.custom_serve(move || RecordingHttpServer::new(NoopAcceptor::new(), service.clone()))
            .await
    }

    async fn custom_serve<F, A>(self, make_server: F) -> io::Result<()>
    where
        F: Fn() -> A,
        A: Serve + Send + Sync + 'static,
    {
        if self.addrs.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "bind or bind_rustls is not set",
            ));
        }

        let mut fut_list = FuturesUnordered::new();

        if !self.addrs.is_empty() {
            let http_server = make_server();

            let addrs = collect_addrs(self.addrs).await.unwrap()?;

            for addr in addrs {
                fut_list.push(http_server.serve_on(addr));
            }
        }

        while let Some(handle) = fut_list.next().await {
            if let Err(e) = handle.unwrap() {
                fut_list.iter().for_each(|handle| handle.abort());

                return Err(e);
            }
        }

        Ok(())
    }
}

/// Shortcut for creating [`Server`](Server::new) and calling [`bind`](Server::bind) on it.
pub fn bind<A>(addr: A) -> Server
where
    A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
{
    Server::new().bind(addr)
}

/// Shortcut for creating [`Server`](Server::new) and calling [`bind_rustls`](Server::bind_rustls) on it.
#[cfg(feature = "tls-rustls")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub fn bind_rustls<A>(addr: A) -> TlsServer
where
    A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
{
    Server::new().bind_rustls(addr)
}

#[derive(Clone)]
pub(crate) struct NoopAcceptor;

impl NoopAcceptor {
    fn new() -> Self {
        Self
    }
}

impl<I> Accept<I> for NoopAcceptor
where
    I: AsyncRead + AsyncWrite + Unpin,
{
    type Stream = I;
    type Future = Ready<io::Result<Self::Stream>>;

    fn accept(&self, stream: I) -> Self::Future {
        futures_util::future::ready(Ok(stream))
    }
}

pub(crate) fn collect_addrs(
    addrs: Vec<BoxedToSocketAddrs>,
) -> JoinHandle<io::Result<Vec<SocketAddr>>> {
    spawn_blocking(move || {
        let mut vec = Vec::new();

        for addrs in addrs {
            let mut iter = addrs.to_socket_addrs()?;

            while let Some(addr) = iter.next() {
                vec.push(addr);
            }
        }

        Ok(vec)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bind_addresses() {
        let server = bind("127.0.0.1:3000").bind("127.0.0.1:3001");

        let addrs = collect_addrs(server.addrs).await.unwrap().unwrap();

        assert_eq!(addrs, [addr("127.0.0.1:3000"), addr("127.0.0.1:3001")]);
    }

    fn addr(s: &'static str) -> SocketAddr {
        s.parse().unwrap()
    }
}
