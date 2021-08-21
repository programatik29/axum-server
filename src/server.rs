use std::{
    io::{self, ErrorKind},
    net::{SocketAddr, ToSocketAddrs},
};

#[cfg(feature = "rustls")]
use std::{
    fs::File,
    io::{BufRead, BufReader, Cursor, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

use futures_util::stream::{FuturesUnordered, StreamExt};

use tower_http::add_extension::AddExtension;

use http::request::Request;
use http::response::Response;
use http_body::Body;

use hyper::server::conn::Http;

use tokio::net::TcpListener;
use tokio::task::{spawn_blocking, JoinHandle};

#[cfg(feature = "rustls")]
use tokio_rustls::{
    rustls::{
        internal::pemfile::certs, internal::pemfile::pkcs8_private_keys,
        internal::pemfile::rsa_private_keys, Certificate, NoClientAuth, PrivateKey, ServerConfig,
    },
    TlsAcceptor,
};

use tower_service::Service;

type BoxedToSocketAddrs = Box<dyn ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send>;

/// Configurable HTTP or HTTPS server, supporting HTTP/1.1 and HTTP2.
#[derive(Default)]
pub struct Server {
    addrs: Vec<BoxedToSocketAddrs>,
    tls_addrs: Vec<BoxedToSocketAddrs>,
    #[cfg(feature = "rustls")]
    private_key: Option<JoinHandle<io::Result<PrivateKey>>>,
    #[cfg(feature = "rustls")]
    certificates: Option<JoinHandle<io::Result<Vec<Certificate>>>>,
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
    pub fn bind_rustls<A>(mut self, addr: A) -> Self
    where
        A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
    {
        self.tls_addrs.push(Box::new(addr));
        self
    }

    /// Set a private key in PEM format.
    ///
    /// Successive calls will overwrite latest private key.
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn private_key(mut self, pem_key: Vec<u8>) -> Self {
        let handle = spawn_blocking(move || {
            let mut reader = Cursor::new(pem_key);

            Ok(load_private_key(&mut reader)?)
        });

        self.private_key = Some(handle);
        self
    }

    /// Set *concatenated certificate and chain*(fullchain) in PEM format.
    ///
    /// Successive calls will overwrite latest certificate.
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn certificate(mut self, pem_cert: Vec<u8>) -> Self {
        let handle = spawn_blocking(move || {
            let mut reader = Cursor::new(pem_cert);

            Ok(load_certificates(&mut reader)?)
        });

        self.certificates = Some(handle);
        self
    }

    /// Set a private key from file in PEM format.
    ///
    /// Successive calls will overwrite latest private key.
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn private_key_file(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_owned();
        let handle = spawn_blocking(move || {
            let mut reader = BufReader::new(File::open(path)?);

            Ok(load_private_key(&mut reader)?)
        });

        self.private_key = Some(handle);
        self
    }

    /// Set *concatenated certificate and chain*(fullchain) from file in PEM format.
    ///
    /// Successive calls will overwrite latest certificate.
    #[cfg(feature = "rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    pub fn certificate_file(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_owned();
        let handle = spawn_blocking(move || {
            let mut reader = BufReader::new(File::open(path)?);

            Ok(load_certificates(&mut reader)?)
        });

        self.certificates = Some(handle);
        self
    }

    /// Serve provided cloneable service on all binded addresses.
    ///
    /// Returns error if accepting connection fails in any one of binded addresses.
    pub async fn serve<S, B>(self, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Clone + 'static,
        S::Error: std::error::Error + Send + Sync,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: std::error::Error + Send + Sync,
    {
        if self.addrs.is_empty() && self.tls_addrs.is_empty() {
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

        #[cfg(feature = "rustls")]
        if !self.tls_addrs.is_empty() {
            let key = self.private_key.ok_or(io::Error::new(
                ErrorKind::InvalidInput,
                "private_key or private_key_file is not set",
            ))?;
            let key = key.await.unwrap()?;

            let certs = self.certificates.ok_or(io::Error::new(
                ErrorKind::InvalidInput,
                "certificates or certificates_file is not set",
            ))?;
            let certs = certs.await.unwrap()?;

            let config = rustls_config(key, certs)?;
            let acceptor = TlsAcceptor::from(config);

            let addrs = collect_addrs(self.tls_addrs).await.unwrap()?;

            for addr in addrs {
                fut_list.push(https_task(addr, acceptor.clone(), service.clone()));
            }
        }

        while let Some(handle) = fut_list.next().await {
            handle.unwrap()?;
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
pub fn bind_rustls<A>(addr: A) -> Server
where
    A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
{
    Server::new().bind_rustls(addr)
}

#[cfg(feature = "rustls")]
fn load_private_key<R: BufRead + Seek>(reader: &mut R) -> io::Result<PrivateKey> {
    let key = pkcs8_private_keys(reader)
        .and_then(try_get_first_key)
        .or_else(|_| {
            reader.seek(SeekFrom::Start(0)).unwrap();
            rsa_private_keys(reader).and_then(try_get_first_key)
        })
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid private key"))?;

    Ok(key)
}

#[cfg(feature = "rustls")]
fn try_get_first_key(mut keys: Vec<PrivateKey>) -> Result<PrivateKey, ()> {
    if !keys.is_empty() {
        Ok(keys.remove(0))
    } else {
        Err(())
    }
}

#[cfg(feature = "rustls")]
fn load_certificates(reader: &mut dyn BufRead) -> io::Result<Vec<Certificate>> {
    let certs =
        certs(reader).map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid certificate"))?;

    Ok(certs)
}

fn collect_addrs(addrs: Vec<BoxedToSocketAddrs>) -> JoinHandle<io::Result<Vec<SocketAddr>>> {
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

fn http_task<S, B>(addr: SocketAddr, service: S) -> JoinHandle<io::Result<()>>
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
                let _ = Http::new().serve_connection(stream, svc).await;
            });
        }
    })
}

#[cfg(feature = "rustls")]
fn rustls_config(
    private_key: PrivateKey,
    certificates: Vec<Certificate>,
) -> io::Result<Arc<ServerConfig>> {
    let mut config = ServerConfig::new(NoClientAuth::new());

    config
        .set_single_cert(certificates, private_key)
        .map_err(|e| io::Error::new(ErrorKind::Other, e))?;

    config.set_protocols(&[b"h2".to_vec(), b"http/1.1".to_vec()]);

    Ok(Arc::new(config))
}

#[cfg(feature = "rustls")]
fn https_task<S, B>(
    addr: SocketAddr,
    acceptor: TlsAcceptor,
    service: S,
) -> JoinHandle<io::Result<()>>
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
            let acceptor = acceptor.clone();

            let svc = AddExtension::new(service.clone(), addr);

            tokio::spawn(async move {
                if let Ok(stream) = acceptor.accept(stream).await {
                    let _ = Http::new().serve_connection(stream, svc).await;
                }
            });
        }
    })
}
