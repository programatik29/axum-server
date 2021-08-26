mod serve;

#[cfg(feature = "tls-rustls")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub mod tls;

#[cfg(feature = "record")]
#[cfg_attr(docsrs, doc(cfg(feature = "record")))]
pub mod record;

use serve::{Accept, HttpServer, Serve};

#[cfg(feature = "tls-rustls")]
use tls::{TlsLoader, TlsServer};

#[cfg(feature = "record")]
use record::RecordingHttpServer;

#[cfg(feature = "tls-rustls")]
use std::path::Path;

#[cfg(feature = "tls-rustls")]
use rustls::ServerConfig;

use std::io;
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use futures_util::future::Ready;
use futures_util::stream::{FuturesUnordered, StreamExt};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Notify;
use tokio::task::{spawn_blocking, JoinHandle};

use tower_service::Service;

use http::request::Request;
use http::response::Response;
use http_body::Body;

pub(crate) type BoxedToSocketAddrs =
    Box<dyn ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send>;

/// A struct that can be passed to a server for additional utilites.
///
/// # Example
///
/// ```rust,no_run
/// use axum::{
///     handler::get,
///     Router,
/// };
/// use axum_server::Handle;
/// use tokio::time::{sleep, Duration};
///
/// #[tokio::main]
/// async fn main() {
///     let app = Router::new().route("/", get(|| async { "Hello, World!" }));
///
///     let handle = Handle::new();
///
///     tokio::spawn(shutdown_in_twenty_secs(handle.clone()));
///
///     axum_server::bind("127.0.0.1:3000")
///         .handle(handle)
///         .serve(app)
///         .await
///         .unwrap();
/// }
///
/// async fn shutdown_in_twenty_secs(handle: Handle) {
///     sleep(Duration::from_secs(20)).await;
///
///     handle.shutdown();
/// }
/// ```
#[derive(Default, Clone)]
pub struct Handle {
    listening: Arc<Notify>,
    shutdown: Arc<Notify>,
    graceful_shutdown: Arc<Notify>,
}

impl Handle {
    /// Create a `Handle`.
    pub fn new() -> Self {
        Handle::default()
    }

    /// Wait until server starts listening.
    pub async fn listening(&self) {
        self.listening.notified().await;
    }

    /// Signal server to shut down.
    ///
    /// [`serve`](Server::serve) function will return when shutdown is complete.
    pub fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// Signal server to gracefully shut down.
    ///
    /// [`serve`](Server::serve) function will return when graceful shutdown is complete.
    pub fn graceful_shutdown(&self) {
        self.graceful_shutdown.notify_waiters();
    }
}

/// Configurable HTTP server, supporting HTTP/1.1 and HTTP2.
///
/// `Server` can conveniently be turned into a [`TlsServer`](TlsServer) with related methods.
///
/// See [main](crate) page for HTTP example. See [`axum_server::tls`](tls) module for HTTPS example.
#[derive(Default)]
pub struct Server {
    addrs: Vec<BoxedToSocketAddrs>,
    handle: Handle,
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

    /// Provide a **loaded** [`TlsLoader`](TlsLoader).
    ///
    /// This will overwrite any previously set private key and certificate(s) with its own ones.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn loader(self, config: TlsLoader) -> TlsServer {
        TlsServer::from(self).loader(config)
    }

    /// Provide [`ServerConfig`](ServerConfig) containing private key and certificate(s).
    ///
    /// When this value is set, other tls configurations are ignored.
    ///
    /// Successive calls will overwrite last value.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn tls_config(self, config: Arc<ServerConfig>) -> TlsServer {
        TlsServer::from(self).tls_config(config)
    }

    /// Set private key in PEM format.
    ///
    /// Successive calls will overwrite last private key.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn private_key(self, key: Vec<u8>) -> TlsServer {
        TlsServer::from(self).private_key(key)
    }

    /// Set certificate(s) in PEM format.
    ///
    /// Successive calls will overwrite last certificate.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn certificate(self, cert: Vec<u8>) -> TlsServer {
        TlsServer::from(self).certificate(cert)
    }

    /// Set private key from file in PEM format.
    ///
    /// Successive calls will overwrite last private key.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn private_key_file(self, path: impl AsRef<Path>) -> TlsServer {
        TlsServer::from(self).private_key_file(path)
    }

    /// Set certificate(s) from file in PEM format.
    ///
    /// Successive calls will overwrite last certificate.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn certificate_file(self, path: impl AsRef<Path>) -> TlsServer {
        TlsServer::from(self).certificate_file(path)
    }

    /// Provide a `Handle`.
    ///
    /// Successive calls will overwrite last `Handle`.
    pub fn handle(mut self, handle: Handle) -> Self {
        self.handle = handle;
        self
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
        self.custom_serve(move |handle| {
            HttpServer::new(NoopAcceptor::new(), service.clone(), handle)
        })
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
        self.custom_serve(move |handle| {
            RecordingHttpServer::new(NoopAcceptor::new(), service.clone(), handle)
        })
        .await
    }

    async fn custom_serve<F, A>(self, make_server: F) -> io::Result<()>
    where
        F: Fn(Handle) -> A,
        A: Serve + Send + Sync + 'static,
    {
        if self.addrs.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "no address provided to bind",
            ));
        }

        let mut fut_list = FuturesUnordered::new();

        if !self.addrs.is_empty() {
            let http_server = make_server(self.handle);

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
