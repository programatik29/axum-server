use std::{
    io::{self, BufReader, ErrorKind, Seek, SeekFrom},
    fs::File,
    path::Path,
    sync::Arc
};

use tower_http::add_extension::AddExtension;

use http::request::Request;
use http::response::Response;
use http_body::Body;

use hyper::server::conn::Http;

use tokio::net::{TcpListener, ToSocketAddrs};
use tokio_rustls::{
    rustls::{
        internal::pemfile::certs, internal::pemfile::pkcs8_private_keys,
        internal::pemfile::rsa_private_keys, NoClientAuth, PrivateKey, ServerConfig,
    },
    TlsAcceptor,
};

use tower_service::Service;

pub struct Server {
    listener: TcpListener,
    tls_acceptor: Option<TlsAcceptor>,
}

impl Server {
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(addr).await?,
            tls_acceptor: None,
        })
    }

    pub async fn bind_rustls<A: ToSocketAddrs>(
        addr: A,
        key_path: impl AsRef<Path>,
        cert_path: impl AsRef<Path>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let tls_acceptor = TlsAcceptor::from(rustls_config(key_path, cert_path)?);

        Ok(Self {
            listener,
            tls_acceptor: Some(tls_acceptor),
        })
    }

    pub async fn serve<S, B>(&self, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Clone + 'static,
        S::Error: std::error::Error + Send + Sync,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: std::error::Error + Send + Sync,
    {
        match &self.tls_acceptor {
            Some(acceptor) => Ok(self.serve_rustls(acceptor, service).await?),
            None => Ok(self.serve_plain(service).await?),
        }
    }

    async fn serve_plain<S, B>(&self, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Clone + 'static,
        S::Error: std::error::Error + Send + Sync,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: std::error::Error + Send + Sync,
    {
        loop {
            let (stream, addr) = self.listener.accept().await?;

            let svc = AddExtension::new(service.clone(), addr);

            tokio::spawn(async move {
                let _ = Http::new().serve_connection(stream, svc).await;
            });
        }
    }

    async fn serve_rustls<S, B>(&self, acceptor: &TlsAcceptor, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Clone + 'static,
        S::Error: std::error::Error + Send + Sync,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: std::error::Error + Send + Sync,
    {
        loop {
            let (stream, addr) = self.listener.accept().await?;
            let acceptor = acceptor.clone();

            let svc = AddExtension::new(service.clone(), addr);

            tokio::spawn(async move {
                if let Ok(stream) = acceptor.accept(stream).await {
                    let _ = Http::new().serve_connection(stream, svc).await;
                }
            });
        }
    }
}

fn rustls_config(key: impl AsRef<Path>, cert: impl AsRef<Path>) -> io::Result<Arc<ServerConfig>> {
    let mut config = ServerConfig::new(NoClientAuth::new());

    let mut key_reader = BufReader::new(File::open(key)?);
    let mut cert_reader = BufReader::new(File::open(cert)?);

    let key = pkcs8_private_keys(&mut key_reader)
        .and_then(try_get_first_key)
        .or_else(|_| {
            key_reader.seek(SeekFrom::Start(0)).unwrap();
            rsa_private_keys(&mut key_reader).and_then(try_get_first_key)
        })
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid private key"))?;

    let certs = certs(&mut cert_reader)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid certificate"))?;

    config
        .set_single_cert(certs, key)
        .map_err(|e| io::Error::new(ErrorKind::Other, e))?;

    config.set_protocols(&[b"h2".to_vec(), b"http/1.1".to_vec()]);

    Ok(Arc::new(config))
}

fn try_get_first_key(mut keys: Vec<PrivateKey>) -> Result<PrivateKey, ()> {
    if !keys.is_empty() {
        Ok(keys.remove(0))
    } else {
        Err(())
    }
}
