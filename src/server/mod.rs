use self::http_server::HttpServer;
use crate::util::HyperService;
use futures_util::stream::{FuturesUnordered, StreamExt};
use http::{uri::Scheme, Request, Response};
use http_body::Body;
use parking_lot::RwLock;
use std::{
    future::Future,
    io::{self, ErrorKind},
    net::{SocketAddr, ToSocketAddrs},
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::{spawn_blocking, JoinHandle},
};
use tower_layer::Layer;
use tower_service::Service;

#[cfg(feature = "tls-rustls")]
use {
    rustls::ServerConfig,
    std::path::Path,
    tls::{TlsLoader, TlsServer},
};

mod http_server;
mod socket_addrs;

#[cfg(feature = "tls-rustls")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub mod tls;

#[cfg(feature = "record")]
#[cfg_attr(docsrs, doc(cfg(feature = "record")))]
pub mod record;

type ListenerTask = JoinHandle<io::Result<()>>;
type FutList = FuturesUnordered<ListenerTask>;

/// Configurable HTTP server, supporting HTTP/1.1 and HTTP2.
///
/// `Server` can conveniently be turned into a [`TlsServer`](TlsServer) with related methods.
///
/// See [main](crate) page for HTTP example. See [`axum_server::tls`](tls) module for HTTPS example.
#[derive(Debug, Default)]
pub struct Server {
    addrs: Vec<socket_addrs::Boxed>,
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
    pub fn bind<A, I>(mut self, addr: A) -> Self
    where
        A: ToSocketAddrs<Iter = I> + Send + 'static,
        I: Iterator<Item = SocketAddr> + 'static,
    {
        let boxed = socket_addrs::Boxed::new(addr);

        self.addrs.push(boxed);
        self
    }

    /// Bind to a single address or multiple addresses. Using tls protocol on streams.
    ///
    /// Certificate and private key must be set before or after calling this.
    #[cfg(feature = "tls-rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
    pub fn bind_rustls<A, I>(self, addr: A) -> TlsServer
    where
        A: ToSocketAddrs<Iter = I> + Send + 'static,
        I: Iterator<Item = SocketAddr> + 'static,
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
            HttpServer::from_service(Scheme::HTTP, service.clone(), handle)
        })
        .await
    }

    /// Serve provided cloneable service on all binded addresses.
    ///
    /// Record sent and received bytes for each connection **independently**. Sent and
    /// received bytes through a connection can be accessed through [`Request`](Request)
    /// extensions.
    ///
    /// See [`axum_server::record`](crate::server::record) module for examples.
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
            HttpServer::recording_from_service(Scheme::HTTP, service.clone(), handle)
        })
        .await
    }

    async fn custom_serve<F, S, M>(self, make_server: F) -> io::Result<()>
    where
        F: Fn(Handle) -> HttpServer<S, M>,
        S: HyperService<Request<hyper::Body>>,
        M: MakeParts + Clone + Send + Sync + 'static,
        M::Layer: Layer<S> + Clone + Send + Sync + 'static,
        <M::Layer as Layer<S>>::Service: HyperService<Request<hyper::Body>>,
        M::Acceptor: Accept,
    {
        serve(move |mut fut_list| async move {
            self.check_addrs()?;

            if !self.addrs.is_empty() {
                serve_addrs(&mut fut_list, self.handle.clone(), make_server, self.addrs).await?;
            }

            self.handle.notify_listening();

            Ok(fut_list)
        })
        .await
    }

    fn check_addrs(&self) -> io::Result<()> {
        if self.addrs.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "no address provided to bind",
            ));
        }

        Ok(())
    }
}

/// Shortcut for creating [`Server`](Server::new) and calling [`bind`](Server::bind) on it.
pub fn bind<A, I>(addr: A) -> Server
where
    A: ToSocketAddrs<Iter = I> + Send + 'static,
    I: Iterator<Item = SocketAddr> + 'static,
{
    Server::new().bind(addr)
}

/// Shortcut for creating [`Server`](Server::new) and calling [`bind_rustls`](Server::bind_rustls) on it.
#[cfg(feature = "tls-rustls")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub fn bind_rustls<A, I>(addr: A) -> TlsServer
where
    A: ToSocketAddrs<Iter = I> + Send + 'static,
    I: Iterator<Item = SocketAddr> + 'static,
{
    Server::new().bind_rustls(addr)
}

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
#[derive(Clone, Debug, Default)]
pub struct Handle {
    inner: Arc<HandleInner>,
}

#[derive(Debug, Default)]
struct HandleInner {
    listening: Notify,
    listening_addrs: RwLock<Option<Vec<SocketAddr>>>,
    shutdown: Notify,
    graceful_shutdown: Notify,
}

impl Handle {
    /// Create a `Handle`.
    pub fn new() -> Self {
        Handle::default()
    }

    /// Signal server to shut down.
    ///
    /// [`serve`](Server::serve) function will return when shutdown is complete.
    pub fn shutdown(&self) {
        self.inner.shutdown.notify_waiters();
    }

    async fn shutdown_signal(&self) {
        self.inner.shutdown.notified().await;
    }

    /// Signal server to gracefully shut down.
    ///
    /// [`serve`](Server::serve) function will return when graceful shutdown is complete.
    pub fn graceful_shutdown(&self) {
        self.inner.graceful_shutdown.notify_waiters();
    }

    async fn graceful_shutdown_signal(&self) {
        self.inner.graceful_shutdown.notified().await;
    }

    /// Get addresses that are being listened.
    ///
    /// Ordered by first `bind` call to last `bind` call.
    /// Then ordered by first `bind_rustls` call to last `bind_rustls` call.
    ///
    /// Always `Some` after [`listening`](Handle::listening) returns.
    pub fn listening_addrs(&self) -> Option<Vec<SocketAddr>> {
        self.inner.listening_addrs.read().clone()
    }

    /// Wait until server starts listening on all addresses.
    pub async fn listening(&self) {
        self.inner.listening.notified().await;
    }

    fn notify_listening(&self) {
        self.inner.listening.notify_waiters();
    }

    fn add_listening_addr(&self, addr: SocketAddr) {
        let mut lock = self.inner.listening_addrs.write();

        match lock.as_mut() {
            Some(vec) => vec.push(addr),
            None => *lock = Some(vec![addr]),
        }
    }
}

pub(crate) trait MakeParts {
    type Layer;
    type Acceptor;

    fn make_parts(&self) -> (Self::Layer, Self::Acceptor);
}

pub(crate) trait Accept<I = TcpStream>: Clone + Send + Sync + 'static {
    type Conn: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type Future: Future<Output = io::Result<Self::Conn>> + Send;

    fn accept(&self, conn: I) -> Self::Future;
}

async fn serve<F, Fut>(fut: F) -> io::Result<()>
where
    F: FnOnce(FutList) -> Fut,
    Fut: Future<Output = io::Result<FutList>>,
{
    let mut fut_list = FuturesUnordered::new();

    fut_list = fut(fut_list).await?;

    while let Some(handle) = fut_list.next().await {
        if let Err(e) = handle.unwrap() {
            fut_list.iter().for_each(|handle| handle.abort());

            return Err(e);
        }
    }

    Ok(())
}

async fn serve_addrs<F, S, M>(
    fut_list: &mut FutList,
    handle: Handle,
    make_server: F,
    addrs: Vec<socket_addrs::Boxed>,
) -> io::Result<()>
where
    F: Fn(Handle) -> HttpServer<S, M>,
    S: HyperService<Request<hyper::Body>>,
    M: MakeParts + Clone + Send + Sync + 'static,
    M::Layer: Layer<S> + Clone + Send + Sync + 'static,
    <M::Layer as Layer<S>>::Service: HyperService<Request<hyper::Body>>,
    M::Acceptor: Accept,
{
    let http_server = make_server(handle.clone());

    let addrs = collect_addrs(addrs).await.unwrap()?;

    for addr in addrs {
        let listener = TcpListener::bind(addr).await?;
        handle.add_listening_addr(listener.local_addr()?);

        fut_list.push(http_server.serve_on(listener));
    }

    Ok(())
}

pub(crate) fn collect_addrs(
    addrs: Vec<socket_addrs::Boxed>,
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
pub(crate) mod tests {
    use super::{bind, collect_addrs, Handle, Server};
    use axum::handler::get;
    use http::{Request, Response};
    use hyper::{
        client::conn::{handshake, SendRequest},
        Body,
    };
    use std::{io, net::SocketAddr};
    use tokio::{net::TcpStream, task::JoinHandle};
    use tower_service::Service;
    use tower_util::ServiceExt;

    #[tokio::test]
    async fn test_bind_addresses() {
        let server = bind("127.0.0.1:3000").bind("127.0.0.1:3001");

        let addrs = collect_addrs(server.addrs).await.unwrap().unwrap();

        assert_eq!(addrs, [addr("127.0.0.1:3000"), addr("127.0.0.1:3001")]);
    }

    fn addr(s: &'static str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn test_listening_addrs() {
        let app = get(|| async { "Hello, world!" });

        let handle = Handle::new();
        let server = Server::new()
            .bind("127.0.0.1:0")
            .bind("127.0.0.1:0")
            .handle(handle.clone())
            .serve(app);

        tokio::spawn(server);

        handle.listening().await;
        assert!(handle.listening_addrs().is_some());
    }

    #[tokio::test]
    async fn test_with_requests() {
        let app = get(|| async { "Hello, world!" });

        let handle = Handle::new();
        let server = Server::new()
            .bind("127.0.0.1:0")
            .bind("127.0.0.1:0")
            .handle(handle.clone())
            .serve(app);

        tokio::spawn(server);

        handle.listening().await;

        let addrs = handle.listening_addrs().unwrap();
        println!("listening addrs: {:?}", addrs);

        for addr in addrs {
            let mut client = http_client(addr).await.unwrap();

            let req = empty_request(&format!("http://127.0.0.1:{}/", addr.port()));

            let resp = client.ready_and().await.unwrap().call(req).await.unwrap();

            assert_eq!(into_text(resp).await, "Hello, world!");
        }
    }

    #[tokio::test]
    async fn test_shutdown() {
        let app = get(|| async { "Hello, world!" });

        let handle = Handle::new();
        let server = Server::new()
            .bind("127.0.0.1:0")
            .bind("127.0.0.1:0")
            .handle(handle.clone())
            .serve(app);

        let server_task = tokio::spawn(server);

        handle.listening().await;

        let addrs = handle.listening_addrs().unwrap();
        println!("listening addrs: {:?}", addrs);

        let addr = addrs[0];
        let mut client = http_client(addr).await.unwrap();

        let req = empty_request(&format!("http://127.0.0.1:{}/", addr.port()));

        let resp = client.ready_and().await.unwrap().call(req).await.unwrap();

        assert_eq!(into_text(resp).await, "Hello, world!");

        handle.shutdown();

        let req = empty_request(&format!("http://127.0.0.1:{}/", addr.port()));

        let resp = client.ready_and().await.unwrap().call(req).await;

        assert!(resp.is_err());

        assert!(server_task.await.unwrap().is_ok());

        for addr in addrs {
            assert!(http_client(addr).await.is_err());
        }
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let app = get(|| async { "Hello, world!" });

        let handle = Handle::new();
        let server = Server::new()
            .bind("127.0.0.1:0")
            .bind("127.0.0.1:0")
            .handle(handle.clone())
            .serve(app);

        let server_task = tokio::spawn(server);

        handle.listening().await;

        let addrs = handle.listening_addrs().unwrap();
        println!("listening addrs: {:?}", addrs);

        let addr = addrs[0];
        let (mut client, conn_handle) = http_client_with_handle(addr).await.unwrap();

        let req = empty_request(&format!("http://127.0.0.1:{}/", addr.port()));

        let resp = client.ready_and().await.unwrap().call(req).await.unwrap();

        assert_eq!(into_text(resp).await, "Hello, world!");

        handle.graceful_shutdown();

        let req = empty_request(&format!("http://127.0.0.1:{}/", addr.port()));

        let resp = client.ready_and().await.unwrap().call(req).await;

        assert!(resp.is_ok());

        for addr in addrs {
            assert!(http_client(addr).await.is_err());
        }

        conn_handle.abort();

        assert!(server_task.await.unwrap().is_ok());
    }

    async fn http_client_with_handle(
        addr: SocketAddr,
    ) -> io::Result<(SendRequest<Body>, JoinHandle<()>)> {
        let stream = TcpStream::connect(format!("127.0.0.1:{}", addr.port())).await?;

        let (client, conn) = handshake(stream)
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        Ok((client, handle))
    }

    pub(crate) async fn http_client(addr: SocketAddr) -> io::Result<SendRequest<Body>> {
        let stream = TcpStream::connect(format!("127.0.0.1:{}", addr.port())).await?;

        let (client, conn) = handshake(stream)
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        tokio::spawn(async move {
            let _ = conn.await;
        });

        Ok(client)
    }

    pub(crate) async fn into_text(resp: Response<Body>) -> String {
        let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub(crate) fn empty_request(uri: &str) -> Request<Body> {
        let mut req = Request::new(Body::empty());

        *req.uri_mut() = uri.parse().unwrap();

        req
    }
}
