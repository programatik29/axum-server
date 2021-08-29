use crate::server::{Accept, Handle, ListenerTask};
use crate::util::HyperService;

use std::io;
use std::net::SocketAddr;

use futures_util::future::Ready;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use tower_http::add_extension::AddExtension;
use tower_layer::Layer;

use hyper::server::conn::Http;
use hyper::Request;

#[derive(Debug)]
enum Mode {
    Shutdown,
    Graceful,
}

#[derive(Clone)]
pub(crate) struct HttpServer<L, S, A> {
    layer: L,
    service: S,
    acceptor: A,
    handle: Handle,
}

impl<L, S, A> HttpServer<L, S, A> {
    pub fn new(service: S, handle: Handle, layer: L, acceptor: A) -> Self {
        Self {
            layer,
            service,
            acceptor,
            handle,
        }
    }
}

impl<S> HttpServer<NoopLayer, S, NoopAcceptor> {
    pub fn from_service(service: S, handle: Handle) -> Self {
        HttpServer::new(service, handle, NoopLayer, NoopAcceptor)
    }
}

#[cfg(feature = "tls-rustls")]
impl<S, A> HttpServer<NoopLayer, S, A> {
    pub fn from_acceptor(service: S, handle: Handle, acceptor: A) -> Self {
        HttpServer::new(service, handle, NoopLayer, acceptor)
    }
}

macro_rules! accept {
    ($handle:expr, $listener:expr) => {
        tokio::select! {
            biased;
            _ = $handle.shutdown_signal() => break Mode::Shutdown,
            _ = $handle.graceful_shutdown_signal() => break Mode::Graceful,
            result = $listener.accept() => result,
        }
    };
}

impl<L, S, A> HttpServer<L, S, A>
where
    L: Layer<AddExtension<S, SocketAddr>> + Clone + Send + Sync + 'static,
    L::Service: HyperService<Request<hyper::Body>>,
    S: HyperService<Request<hyper::Body>>,
    A: Accept + Send + Sync + 'static,
{
    pub fn serve_on(&self, listener: TcpListener) -> ListenerTask {
        let server = self.clone();

        tokio::spawn(async move {
            let mut conns = Vec::new();

            let mode = loop {
                let (stream, addr) = accept!(&server.handle, &listener)?;
                let acceptor = server.acceptor.clone();

                let service = server.service.clone();
                let service = AddExtension::new(service, addr);
                let service = server.layer.layer(service);

                let conn = tokio::spawn(async move {
                    if let Ok(stream) = acceptor.accept(stream).await {
                        let _ = Http::new()
                            .serve_connection(stream, service)
                            .with_upgrades()
                            .await;
                    }
                });

                conns.push(conn);
            };

            drop(listener);

            match mode {
                Mode::Shutdown => shutdown_conns(conns),
                Mode::Graceful => tokio::select! {
                    biased;
                    _ = server.handle.shutdown_signal() => shutdown_conns(conns),
                    _ = wait_conns(&mut conns) => (),
                },
            }

            Ok(())
        })
    }
}

fn shutdown_conns(conns: Vec<JoinHandle<()>>) {
    for conn in conns {
        conn.abort();
    }
}

async fn wait_conns(conns: &mut Vec<JoinHandle<()>>) {
    for conn in conns {
        let _ = conn.await;
    }
}

#[derive(Clone)]
pub struct NoopAcceptor;

impl<I> Accept<I> for NoopAcceptor
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Conn = I;
    type Future = Ready<io::Result<Self::Conn>>;

    fn accept(&self, stream: I) -> Self::Future {
        futures_util::future::ready(Ok(stream))
    }
}

#[derive(Clone)]
pub struct NoopLayer;

impl<S> Layer<S> for NoopLayer {
    type Service = S;

    fn layer(&self, layer: S) -> Self::Service {
        layer
    }
}
