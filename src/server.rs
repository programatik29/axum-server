use crate::{
    accept::{Accept, DefaultAcceptor},
    handle::Handle,
    http_config::HttpConfig,
    service::{MakeServiceRef, SendService},
};
use futures_util::{future::poll_fn, Future};
use http::Request;
use hyper::server::{
    accept::Accept as HyperAccept,
    conn::{AddrIncoming, AddrStream},
};
use pin_project_lite::pin_project;
use std::{
    fmt,
    io::{self, ErrorKind},
    net::SocketAddr,
    pin::Pin,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
};

/// HTTP server builder.
#[derive(Debug)]
pub struct ServerBuilder<A = DefaultAcceptor> {
    acceptor: A,
    addr: SocketAddr,
    handle: Handle,
    http_conf: HttpConfig,
}

/// Create a [`ServerBuilder`] that will bind to provided address.
pub fn bind(addr: SocketAddr) -> ServerBuilder {
    ServerBuilder::new(addr)
}

impl ServerBuilder {
    /// Create a server that will bind to provided address.
    pub fn new(addr: SocketAddr) -> Self {
        let acceptor = DefaultAcceptor::new();
        let handle = Handle::new();

        Self {
            acceptor,
            addr,
            handle,
            http_conf: HttpConfig::default(),
        }
    }
}

impl<A> ServerBuilder<A> {
    /// Overwrite acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> ServerBuilder<Acceptor> {
        ServerBuilder {
            acceptor,
            addr: self.addr,
            handle: self.handle,
            http_conf: self.http_conf,
        }
    }

    /// Provide a handle for additional utilities.
    pub fn handle(mut self, handle: Handle) -> Self {
        self.handle = handle;
        self
    }

    /// Overwrite http configuration.
    pub fn http_config(mut self, config: HttpConfig) -> Self {
        self.http_conf = config;
        self
    }

    /// Serve provided [`MakeService`].
    ///
    /// # Errors
    ///
    /// An error will be returned when:
    ///
    /// - Binding to an address fails.
    /// - `make_service` returns an error when `poll_ready` is called. This never happens on
    /// [`axum`] make services.
    ///
    /// [`axum`]: https://docs.rs/axum/0.3
    /// [`MakeService`]: https://docs.rs/tower/0.4/tower/make/trait.MakeService.html
    pub async fn serve<M>(
        self,
        mut make_service: M,
    ) -> io::Result<Server<impl Future<Output = io::Result<()>>>>
    where
        M: MakeServiceRef<AddrStream, Request<hyper::Body>>,
        A: Accept<AddrStream, M::Service> + Clone + Send + Sync + 'static,
        A::Stream: AsyncRead + AsyncWrite + Unpin + Send,
        A::Service: SendService<Request<hyper::Body>> + Send,
        A::Future: Send,
    {
        let acceptor = self.acceptor;
        let handle = self.handle;
        let http_conf = self.http_conf;

        let listener = TcpListener::bind(self.addr).await?;
        let addr = listener.local_addr()?;
        let fut = async move {
            let mut incoming = AddrIncoming::from_listener(listener).map_err(io_other)?;

            handle.notify_listening(incoming.local_addr());

            let accept_loop_future = async {
                loop {
                    let addr_stream = tokio::select! {
                        biased;
                        result = accept(&mut incoming) => result?,
                        _ = handle.wait_graceful_shutdown() => return Ok(()),
                    };

                    poll_fn(|cx| make_service.poll_ready(cx))
                        .await
                        .map_err(io_other)?;

                    let service = match make_service.make_service(&addr_stream).await {
                        Ok(service) => service,
                        Err(_) => continue,
                    };

                    let acceptor = acceptor.clone();
                    let watcher = handle.watcher();
                    let http_conf = http_conf.clone();

                    tokio::spawn(async move {
                        if let Ok((stream, send_service)) =
                            acceptor.accept(addr_stream, service).await
                        {
                            let service = send_service.into_service();

                            let serve_future = http_conf
                                .inner
                                .serve_connection(stream, service)
                                .with_upgrades();

                            tokio::select! {
                                biased;
                                _ = watcher.wait_shutdown() => (),
                                _ = serve_future => (),
                            }
                        }
                    });
                }
            };

            let result = tokio::select! {
                biased;
                _ = handle.wait_shutdown() => return Ok(()),
                result = accept_loop_future => result,
            };

            if let Err(e) = result {
                return Err(e);
            }

            handle.wait_connections_end().await;

            Ok(())
        };
        Ok(Server { inner: fut, addr })
    }
}

pin_project! {
    /// Ready to run server.
    ///
    /// Must be awaited to start serving requests.
    #[must_use = "you need to await the server to start serving connections"]
    pub struct Server<F> {
        #[pin]
        inner: F,
        addr: SocketAddr,
    }
}

impl<F> Server<F> {
    /// Return the local adress the server is listening on.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

impl<F> Future for Server<F>
where
    F: Future<Output = io::Result<()>>,
{
    type Output = io::Result<()>;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let slf = self.project();
        slf.inner.poll(cx)
    }
}

impl<F> fmt::Debug for Server<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server").field("addr", &self.addr).finish()
    }
}

pub(crate) async fn accept(incoming: &mut AddrIncoming) -> io::Result<AddrStream> {
    let mut incoming = Pin::new(incoming);

    // Always [`Option::Some`].
    // https://docs.rs/hyper/0.14.14/src/hyper/server/tcp.rs.html#165
    poll_fn(|cx| incoming.as_mut().poll_accept(cx))
        .await
        .unwrap()
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) fn io_other<E: Into<BoxError>>(error: E) -> io::Error {
    io::Error::new(ErrorKind::Other, error)
}

#[cfg(test)]
mod tests {
    use crate::{handle::Handle, server::ServerBuilder};
    use axum::{routing::get, Router};
    use bytes::Bytes;
    use http::{response, Request};
    use hyper::{
        client::conn::{handshake, SendRequest},
        Body,
    };
    use std::{io, net::SocketAddr, time::Duration};
    use tokio::{net::TcpStream, task::JoinHandle, time::timeout};
    use tower::{Service, ServiceExt};

    #[tokio::test]
    async fn start_and_request() {
        let (_handle, _server_task, addr) = start_server().await;

        let (mut client, _conn) = connect(addr).await;

        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(body.as_ref(), b"Hello, world!");
    }

    #[tokio::test]
    async fn test_shutdown() {
        let (handle, _server_task, addr) = start_server().await;

        let (mut client, conn) = connect(addr).await;

        handle.shutdown();

        let response_future_result = client
            .ready()
            .await
            .unwrap()
            .call(Request::new(Body::empty()))
            .await;

        assert!(response_future_result.is_err());

        // Connection task should finish soon.
        let _ = timeout(Duration::from_secs(1), conn).await.unwrap();
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let (handle, server_task, addr) = start_server().await;

        let (mut client, conn) = connect(addr).await;

        handle.graceful_shutdown(None);

        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(body.as_ref(), b"Hello, world!");

        // Disconnect client.
        conn.abort();

        // Server task should finish soon.
        let server_result = timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap();

        assert!(server_result.is_ok());
    }

    #[ignore]
    #[tokio::test]
    async fn test_graceful_shutdown_timed() {
        let (handle, server_task, addr) = start_server().await;

        let (mut client, _conn) = connect(addr).await;

        handle.graceful_shutdown(Some(Duration::from_millis(250)));

        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(body.as_ref(), b"Hello, world!");

        // Don't disconnect client.
        // conn.abort();

        // Server task should finish soon.
        let server_result = timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap();

        assert!(server_result.is_ok());
    }

    async fn start_server() -> (Handle, JoinHandle<io::Result<()>>, SocketAddr) {
        let handle = Handle::new();

        let server_handle = handle.clone();
        let server_task = tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            ServerBuilder::new(addr)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await;

        (handle, server_task, addr)
    }

    async fn connect(addr: SocketAddr) -> (SendRequest<Body>, JoinHandle<()>) {
        let stream = TcpStream::connect(addr).await.unwrap();

        let (send_request, connection) = handshake(stream).await.unwrap();

        let task = tokio::spawn(async move {
            let _ = connection.await;
        });

        (send_request, task)
    }

    async fn send_empty_request(client: &mut SendRequest<Body>) -> (response::Parts, Bytes) {
        let (parts, body) = client
            .ready()
            .await
            .unwrap()
            .call(Request::new(Body::empty()))
            .await
            .unwrap()
            .into_parts();
        let body = hyper::body::to_bytes(body).await.unwrap();

        (parts, body)
    }
}
