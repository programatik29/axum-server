use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use http::request::Request;
use http::response::Response;
use http_body::Body;

use tower_http::add_extension::AddExtension;
use tower_service::Service;

use hyper::server::conn::Http;

pub trait Accept<I = TcpStream>: Clone
where
    I: AsyncRead + AsyncWrite + Unpin,
{
    type Stream: AsyncRead + AsyncWrite + Unpin;
    type Future: std::future::Future<Output = io::Result<Self::Stream>>;

    fn accept(&self, stream: I) -> Self::Future;
}

pub trait Serve {
    fn serve_on(&self, addr: SocketAddr) -> JoinHandle<io::Result<()>>;
}

pub struct HttpServer<A, S> {
    acceptor: A,
    service: S,
}

impl<A, S> HttpServer<A, S> {
    pub fn new(acceptor: A, service: S) -> Self {
        Self { acceptor, service }
    }
}

impl<A, S, B> Serve for HttpServer<A, S>
where
    A: Accept + Send + Sync + 'static,
    A::Stream: Send + 'static,
    A::Future: Send + 'static,
    S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Clone + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: Send,
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    fn serve_on(&self, addr: SocketAddr) -> JoinHandle<io::Result<()>> {
        let acceptor = self.acceptor.clone();
        let service = self.service.clone();

        tokio::spawn(async move {
            let listener = TcpListener::bind(addr).await?;

            loop {
                let (stream, addr) = listener.accept().await?;
                let acceptor = acceptor.clone();

                let svc = AddExtension::new(service.clone(), addr);

                tokio::spawn(async move {
                    if let Ok(stream) = acceptor.accept(stream).await {
                        let _ = Http::new()
                            .serve_connection(stream, svc)
                            .with_upgrades()
                            .await;
                    }
                });
            }
        })
    }
}
