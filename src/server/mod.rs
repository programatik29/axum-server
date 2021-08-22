#[cfg(feature = "rustls")]
#[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
pub mod tls;

#[cfg(feature = "rustls")]
use tls::TlsServer;

#[cfg(feature = "rustls")]
use std::path::Path;

use std::{
    io::{self, ErrorKind},
    net::{SocketAddr, ToSocketAddrs},
};

use futures_util::stream::{FuturesUnordered, StreamExt};

use tower_http::add_extension::AddExtension;

use http::request::Request;
use http::response::Response;
use http_body::Body;

use hyper::server::conn::Http;

use tokio::net::TcpListener;
use tokio::task::{spawn_blocking, JoinHandle};

use tower_service::Service;

pub(crate) type BoxedToSocketAddrs = Box<dyn ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send>;

/// Configurable HTTP server, supporting HTTP/1.1 and HTTP2.
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
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn bind_rustls<A>(self, addr: A) -> TlsServer
    where
        A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
    {
        TlsServer::from(self).bind_rustls(addr)
    }

    /// Load private key in PEM format.
    ///
    /// Successive calls will overwrite latest private key.
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn private_key(self, key: Vec<u8>) -> TlsServer {
        TlsServer::from(self).private_key(key)
    }

    /// Load certificate(s) in PEM format.
    ///
    /// Successive calls will overwrite latest certificate.
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn certificate(self, cert: Vec<u8>) -> TlsServer {
        TlsServer::from(self).certificate(cert)
    }

    /// Load private key from file in PEM format.
    ///
    /// Successive calls will overwrite latest private key.
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn private_key_file(self, path: impl AsRef<Path>) -> TlsServer {
        TlsServer::from(self).private_key_file(path)
    }

    /// Load certificate(s) from file in PEM format.
    ///
    /// Successive calls will overwrite latest certificate.
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn certificate_file(self, path: impl AsRef<Path>) -> TlsServer {
        TlsServer::from(self).certificate_file(path)
    }

    /// Serve provided cloneable service on all binded addresses.
    ///
    /// If accepting connection fails in any one of binded addresses, listening in all binded addresses will be stopped and then an error will be returned.
    pub async fn serve<S, B>(self, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Clone + 'static,
        S::Error: std::error::Error + Send + Sync,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: std::error::Error + Send + Sync,
    {
        if self.addrs.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "bind or bind_rustls is not set",
            ));
        }

        let mut fut_list = FuturesUnordered::new();

        if !self.addrs.is_empty() {
            let addrs = collect_addrs(self.addrs).await.unwrap()?;

            for addr in addrs {
                fut_list.push(http_task(addr, service.clone()));
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
#[cfg(feature = "rustls")]
#[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
pub fn bind_rustls<A>(addr: A) -> TlsServer
where
    A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
{
    Server::new().bind_rustls(addr)
}

pub(crate) fn collect_addrs(addrs: Vec<BoxedToSocketAddrs>) -> JoinHandle<io::Result<Vec<SocketAddr>>> {
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

pub(crate) fn http_task<S, B>(addr: SocketAddr, service: S) -> JoinHandle<io::Result<()>>
where
    S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Clone + 'static,
    S::Error: std::error::Error + Send + Sync,
    S::Future: Send,
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: std::error::Error + Send + Sync,
{
    tokio::spawn(async move {
        let listener = TcpListener::bind(addr).await?;

        loop {
            let (stream, addr) = listener.accept().await?;

            let svc = AddExtension::new(service.clone(), addr);

            tokio::spawn(async move {
                let _ = Http::new()
                    .serve_connection(stream, svc)
                    .with_upgrades()
                    .await;
            });
        }
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
