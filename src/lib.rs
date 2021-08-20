use std::{
    fs::File,
    io::{self, BufRead, BufReader, Cursor, ErrorKind, Seek, SeekFrom},
    net::{SocketAddr, ToSocketAddrs},
    path::Path,
    sync::Arc,
};

use futures_util::stream::{StreamExt, FuturesUnordered};

use tower_http::add_extension::AddExtension;

use http::request::Request;
use http::response::Response;
use http_body::Body;

use hyper::server::conn::Http;

use tokio::net::TcpListener;
use tokio::task::{spawn_blocking, JoinHandle};
use tokio_rustls::{
    rustls::{
        internal::pemfile::certs, internal::pemfile::pkcs8_private_keys,
        internal::pemfile::rsa_private_keys, Certificate, NoClientAuth, PrivateKey, ServerConfig,
    },
    TlsAcceptor,
};

use tower_service::Service;

type BoxedToSocketAddrs = Box<dyn ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>>>;

#[derive(Default)]
pub struct Server {
    addrs: Vec<BoxedToSocketAddrs>,
    tls_addrs: Vec<BoxedToSocketAddrs>,
    private_key: Option<JoinHandle<io::Result<PrivateKey>>>,
    certificates: Option<JoinHandle<io::Result<Vec<Certificate>>>>,
}

impl Server {
    pub fn new() -> Self {
        Server::default()
    }

    pub fn bind<A>(mut self, addr: A) -> Self
    where
        A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + 'static,
    {
        self.addrs.push(Box::new(addr));
        self
    }

    pub fn bind_rustls<A>(mut self, addr: A) -> Self
    where
        A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + 'static,
    {
        self.tls_addrs.push(Box::new(addr));
        self
    }

    pub fn private_key(mut self, pem_key: Vec<u8>) -> Self {
        let handle = spawn_blocking(move || {
            let mut reader = Cursor::new(pem_key);

            Ok(load_private_key(&mut reader)?)
        });

        self.private_key = Some(handle);
        self
    }

    pub fn certificate(mut self, pem_cert: Vec<u8>) -> Self {
        let handle = spawn_blocking(move || {
            let mut reader = Cursor::new(pem_cert);

            Ok(load_certificates(&mut reader)?)
        });

        self.certificates = Some(handle);
        self
    }

    pub fn private_key_file(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_owned();
        let handle = spawn_blocking(move || {
            let mut reader = BufReader::new(File::open(path)?);

            Ok(load_private_key(&mut reader)?)
        });

        self.private_key = Some(handle);
        self
    }

    pub fn certificate_file(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_owned();
        let handle = spawn_blocking(move || {
            let mut reader = BufReader::new(File::open(path)?);

            Ok(load_certificates(&mut reader)?)
        });

        self.certificates = Some(handle);
        self
    }

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
            return Err(io::Error::new(ErrorKind::InvalidInput, "bind or bind_rustls is not set"));
        }

        let mut fut_list = FuturesUnordered::new();

        if !self.addrs.is_empty() {
            let addrs = collect_addrs(self.addrs)?;

            for addr in addrs {
                fut_list.push(http_task(addr, service.clone()));
            }
        }

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

            let addrs = collect_addrs(self.tls_addrs)?;

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

pub fn bind<A>(addr: A) -> Server
where
    A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + 'static,
{
    Server::new().bind(addr)
}

pub fn bind_rustls<A>(addr: A) -> Server
where
    A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + 'static,
{
    Server::new().bind_rustls(addr)
}

fn collect_addrs(addrs: Vec<BoxedToSocketAddrs>) -> io::Result<Vec<SocketAddr>> {
    let mut vec = Vec::new();

    for addrs in addrs {
        let mut iter = addrs.to_socket_addrs()?;

        while let Some(addr) = iter.next() {
            vec.push(addr);
        }
    }

    Ok(vec)
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

fn load_certificates(reader: &mut dyn BufRead) -> io::Result<Vec<Certificate>> {
    let certs =
        certs(reader).map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid certificate"))?;

    Ok(certs)
}

fn try_get_first_key(mut keys: Vec<PrivateKey>) -> Result<PrivateKey, ()> {
    if !keys.is_empty() {
        Ok(keys.remove(0))
    } else {
        Err(())
    }
}

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
