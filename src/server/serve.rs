use crate::server::Handle;

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

pub enum Mode {
    Shutdown,
    Graceful,
}

pub struct HttpServer<A, S> {
    acceptor: A,
    service: S,
    handle: Handle,
}

impl<A, S> HttpServer<A, S> {
    pub fn new(acceptor: A, service: S, handle: Handle) -> Self {
        Self {
            acceptor,
            service,
            handle,
        }
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
        let handle = self.handle.clone();

        tokio::spawn(async move {
            let listener = TcpListener::bind(addr).await?;
            handle.listening.notify_waiters();

            let mut connections = Vec::new();
            let mode;

            mode = loop {
                let (stream, addr) = tokio::select! {
                    result = listener.accept() => result?,
                    _ = handle.shutdown.notified() => break Mode::Shutdown,
                    _ = handle.graceful_shutdown.notified() => break Mode::Graceful,
                };
                let acceptor = acceptor.clone();

                let svc = AddExtension::new(service.clone(), addr);

                let conn = tokio::spawn(async move {
                    if let Ok(stream) = acceptor.accept(stream).await {
                        let _ = Http::new()
                            .serve_connection(stream, svc)
                            .with_upgrades()
                            .await;
                    }
                });

                connections.push(conn);
            };

            match mode {
                Mode::Shutdown => shutdown_conns(connections),
                Mode::Graceful => tokio::select! {
                    _ = handle.shutdown.notified() => shutdown_conns(connections),
                    _ = wait_conns(&mut connections) => (),
                },
            }

            Ok(())
        })
    }
}

pub fn shutdown_conns(connections: Vec<JoinHandle<()>>) {
    for conn in connections {
        conn.abort();
    }
}

pub async fn wait_conns(connections: &mut Vec<JoinHandle<()>>) {
    for conn in connections {
        let _ = conn.await;
    }
}
