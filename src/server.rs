use crate::{
    accept::{Accept, DefaultAcceptor},
    handle::Handle,
    service::{MakeService, SendService},
};
use http::Request;
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{
    fmt,
    future::poll_fn,
    io::{self, ErrorKind},
    net::SocketAddr as IpSocketAddr,
    os::unix::net::SocketAddr as UnixSocketAddr,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream, UnixListener, UnixStream},
};

/// HTTP server.
pub struct Server<Addr: Address, A = DefaultAcceptor> {
    acceptor: A,
    builder: Builder<TokioExecutor>,
    listener_conf: ListenerConf<Addr>,
    handle: Handle<Addr>,
}

// Builder doesn't implement Debug or Clone right now
impl<A: Address, B> fmt::Debug for Server<A, B>
where
    ListenerConf<A>: fmt::Debug,
    Handle<A>: fmt::Debug,
    B: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Server")
            .field("acceptor", &self.acceptor)
            .field("listener_conf", &self.listener_conf)
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum ListenerConf<A: Address> {
    Bind(A),
    Ready(A::Listener),
}

/// Create a [`Server`] that will bind to provided address.
pub fn bind<A: Address>(addr: A) -> Server<A> {
    Server::bind(addr)
}

/// Create a [`Server`] from existing `std::net::TcpListener`.
pub fn from_tcp(listener: std::net::TcpListener) -> io::Result<Server<IpSocketAddr>> {
    Ok(Server::from_listener(TcpListener::from_std(listener)?))
}

/// Create a [`Server`] from existing `std::os::unix::net::UnixListener`.
pub fn from_unix(listener: std::os::unix::net::UnixListener) -> io::Result<Server<UnixSocketAddr>> {
    Ok(Server::from_listener(UnixListener::from_std(listener)?))
}

/// A trait to abstract over a listener that can accept an incoming connection of different types
/// and can be created from an address.
pub trait AddrListener<Stream, Addr: Address>: std::marker::Sized {
    /// Create a new `AddrListener` bound to the given address.
    fn bind_to(addr: Addr) -> impl std::future::Future<Output = io::Result<Self>> + Send;

    /// Accept an incoming connection, returning the stream and the remote address.
    fn accept_stream(&self)
        -> impl std::future::Future<Output = io::Result<(Stream, Addr)>> + Send;

    /// Get the address we're listening on.
    fn get_local_addr(&self) -> io::Result<Addr>;
}

impl AddrListener<UnixStream, UnixSocketAddr> for UnixListener {
    async fn bind_to(addr: UnixSocketAddr) -> io::Result<Self> {
        UnixListener::bind(addr.as_pathname().ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidInput,
                "A UnixListener can only be bound to a path address!",
            )
        })?)
    }

    async fn accept_stream(&self) -> io::Result<(UnixStream, UnixSocketAddr)> {
        let (stream, tokio_addr) = self.accept().await?;
        Ok((stream, tokio_addr.into()))
    }

    fn get_local_addr(&self) -> io::Result<UnixSocketAddr> {
        self.local_addr().map(tokio::net::unix::SocketAddr::into)
    }
}

impl AddrListener<TcpStream, IpSocketAddr> for TcpListener {
    async fn bind_to(addr: IpSocketAddr) -> io::Result<Self> {
        TcpListener::bind(addr).await
    }

    async fn accept_stream(&self) -> io::Result<(TcpStream, IpSocketAddr)> {
        self.accept().await
    }

    fn get_local_addr(&self) -> io::Result<IpSocketAddr> {
        self.local_addr()
    }
}

/// An address which an associated `AddrListener` can bind to.
pub trait Address: std::marker::Sized + Clone {
    /// The underlying stream we will obtain once an `AddrListener` is bound to this address.
    type Stream;

    /// An `AddrListener` that can bind to this `Address`.
    type Listener: AddrListener<Self::Stream, Self>;
}

impl Address for UnixSocketAddr {
    type Stream = UnixStream;
    type Listener = UnixListener;
}

impl Address for IpSocketAddr {
    type Stream = TcpStream;
    type Listener = TcpListener;
}

impl<A: Address> Server<A> {
    /// Create a server that will bind to provided address.
    pub fn bind(addr: A) -> Self {
        let acceptor = DefaultAcceptor::new();
        let builder = Builder::new(TokioExecutor::new());
        let handle = Handle::new();

        Self {
            acceptor,
            builder,
            listener_conf: ListenerConf::Bind(addr),
            handle,
        }
    }

    /// Create a server from an existing `AddrListener`.
    pub fn from_listener(listener: A::Listener) -> Self {
        let acceptor = DefaultAcceptor::new();
        let builder = Builder::new(TokioExecutor::new());
        let handle = Handle::new();

        Self {
            acceptor,
            builder,
            listener_conf: ListenerConf::Ready(listener),
            handle,
        }
    }
}

impl<A: Address, Acc> Server<A, Acc> {
    /// Overwrite acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> Server<A, Acceptor> {
        Server {
            acceptor,
            builder: self.builder,
            listener_conf: self.listener_conf,
            handle: self.handle,
        }
    }

    /// Map acceptor.
    pub fn map<Acceptor, F>(self, acceptor: F) -> Server<A, Acceptor>
    where
        F: FnOnce(Acc) -> Acceptor,
    {
        Server {
            acceptor: acceptor(self.acceptor),
            builder: self.builder,
            listener_conf: self.listener_conf,
            handle: self.handle,
        }
    }

    /// Returns a reference to the acceptor.
    pub fn get_ref(&self) -> &Acc {
        &self.acceptor
    }

    /// Returns a mutable reference to the acceptor.
    pub fn get_mut(&mut self) -> &mut Acc {
        &mut self.acceptor
    }

    /// Returns a mutable reference to the Http builder.
    pub fn http_builder(&mut self) -> &mut Builder<TokioExecutor> {
        &mut self.builder
    }

    /// Provide a handle for additional utilities.
    pub fn handle(mut self, handle: Handle<A>) -> Self {
        self.handle = handle;
        self
    }

    /// Serve provided [`MakeService`].
    ///
    /// To create [`MakeService`] easily, `Shared` from [`tower`] can be used.
    ///
    /// # Errors
    ///
    /// An error will be returned when:
    ///
    /// - Binding to an address fails.
    /// - `make_service` returns an error when `poll_ready` is called. This never happens on
    ///   [`axum`] make services.
    ///
    /// [`axum`]: https://docs.rs/axum/0.3
    /// [`tower`]: https://docs.rs/tower
    /// [`MakeService`]: https://docs.rs/tower/0.4/tower/make/trait.MakeService.html
    pub async fn serve<M>(self, mut make_service: M) -> io::Result<()>
    where
        M: MakeService<A, Request<Incoming>>,
        A: Send + 'static,
        A::Stream: Send,
        Acc: Accept<A::Stream, M::Service> + Clone + Send + Sync + 'static,
        Acc::Stream: AsyncRead + AsyncWrite + Unpin + Send,
        Acc::Service: SendService<Request<Incoming>> + Send,
        Acc::Future: Send,
    {
        let acceptor = self.acceptor;
        let handle = self.handle;
        let builder = std::sync::Arc::new(self.builder);

        let mut incoming = match bind_incoming(self.listener_conf).await {
            Ok(v) => v,
            Err(e) => {
                handle.notify_listening(None);
                return Err(e);
            }
        };

        handle.notify_listening(incoming.get_local_addr().ok());

        let accept_loop_future = async {
            loop {
                let (tcp_stream, socket_addr) = tokio::select! {
                    biased;
                    result = accept(&mut incoming) => result,
                    _ = handle.wait_graceful_shutdown() => return Ok(()),
                };

                poll_fn(|cx| make_service.poll_ready(cx))
                    .await
                    .map_err(io_other)?;

                let service = match make_service.make_service(socket_addr).await {
                    Ok(service) => service,
                    Err(_) => continue,
                };

                let acceptor = acceptor.clone();
                let watcher = handle.watcher();
                let builder = builder.clone();

                tokio::spawn(async move {
                    if let Ok((stream, send_service)) = acceptor.accept(tcp_stream, service).await {
                        let io = TokioIo::new(stream);
                        let service = send_service.into_service();
                        let service = TowerToHyperService::new(service);

                        let serve_future = builder.serve_connection_with_upgrades(io, service);
                        tokio::pin!(serve_future);

                        tokio::select! {
                            biased;
                            _ = watcher.wait_graceful_shutdown() => {
                                serve_future.as_mut().graceful_shutdown();
                                tokio::select! {
                                    biased;
                                    _ = watcher.wait_shutdown() => (),
                                    _ = &mut serve_future => (),
                                }
                            }
                            _ = watcher.wait_shutdown() => (),
                            _ = &mut serve_future => (),
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

        // Tokio internally accepts TCP connections while the TCPListener is active;
        // drop the listener to immediately refuse connections rather than letting
        // them hang.
        drop(incoming);

        // attempting to do a "result?;" requires us to specify the type of result which is annoying
        #[allow(clippy::question_mark)]
        if let Err(e) = result {
            return Err(e);
        }

        handle.wait_connections_end().await;

        Ok(())
    }
}

async fn bind_incoming<A: Address>(listener: ListenerConf<A>) -> io::Result<A::Listener> {
    match listener {
        ListenerConf::Bind(addr) => A::Listener::bind_to(addr).await,
        ListenerConf::Ready(listener) => Ok(listener),
    }
}

pub(crate) async fn accept<L: AddrListener<S, A>, S, A: Address>(listener: &mut L) -> (S, A) {
    loop {
        match listener.accept_stream().await {
            Ok(value) => return value,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) fn io_other<E: Into<BoxError>>(error: E) -> io::Error {
    io::Error::new(ErrorKind::Other, error)
}

#[cfg(test)]
mod tests {
    use crate::{handle::Handle, server::Server};
    use axum::body::Body;
    use axum::response::Response;
    use axum::routing::post;
    use axum::{routing::get, Router};
    use bytes::Bytes;
    use futures_util::{stream, StreamExt};
    use http::{Method, Request, Uri};
    use http_body::Frame;
    use http_body_util::{BodyExt, StreamBody};
    use hyper::client::conn::http1::handshake;
    use hyper::client::conn::http1::SendRequest;
    use hyper_util::rt::TokioIo;
    use std::{io, net::SocketAddr as IpSocketAddr, time::Duration};
    use tokio::sync::oneshot;
    use tokio::{net::TcpStream, task::JoinHandle, time::timeout};

    #[tokio::test]
    async fn start_and_request() {
        let (_handle, _server_task, addr) = start_server().await;

        let (mut client, _conn) = connect(addr).await;

        // Client can send requests

        do_empty_request(&mut client).await.unwrap();

        do_slow_request(&mut client, Duration::from_millis(50))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_shutdown() {
        let (handle, _server_task, addr) = start_server().await;

        let (mut client, conn) = connect(addr).await;

        // Client can send request before shutdown.
        do_empty_request(&mut client).await.unwrap();

        handle.shutdown();

        // After shutdown, all client requests should fail.
        do_empty_request(&mut client).await.unwrap_err();

        // Connection should finish soon.
        let _ = timeout(Duration::from_secs(1), conn).await.unwrap();
    }

    // Test graceful shutdown with no timeout.
    #[tokio::test]
    async fn test_graceful_shutdown_no_timeout() {
        let (handle, server_task, addr) = start_server().await;

        let (mut client1, _conn1) = connect(addr).await;
        let (mut client2, _conn2) = connect(addr).await;

        // Clients can send request before graceful shutdown.
        do_empty_request(&mut client1).await.unwrap();
        do_empty_request(&mut client2).await.unwrap();

        let start = tokio::time::Instant::now();

        let (hdr1_tx, hdr1_rx) = oneshot::channel::<()>();

        let fut1 = async {
            // A slow request made before graceful shutdown is handled.
            // Since there's no request timeout, this can take as long as it
            // needs.
            let hdr1 = send_slow_request(&mut client1, Duration::from_millis(500))
                .await
                .unwrap();
            hdr1_tx.send(()).unwrap();
            recv_slow_response_body(hdr1).await.unwrap();

            assert!(start.elapsed() >= Duration::from_millis(500));
        };
        let fut2 = async {
            // Graceful shutdown partway through
            tokio::time::sleep(Duration::from_millis(250)).await;
            hdr1_rx.await.unwrap();
            handle.graceful_shutdown(None);

            // Any new requests after graceful shutdown begins will fail
            do_empty_request(&mut client2).await.unwrap_err();
            do_empty_request(&mut client2).await.unwrap_err();
            do_empty_request(&mut client2).await.unwrap_err();
        };

        tokio::join!(fut1, fut2);

        // At this point, graceful shutdown must have occured, and the slow
        // request must have finished. Since there was no timeout, the elapsed
        // time should be at least 500 ms (slow request duration).
        assert!(start.elapsed() >= Duration::from_millis(500 + 100));

        // Server task should finish soon.
        timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    // Test graceful shutdown with a timeout.
    #[tokio::test]
    async fn test_graceful_shutdown_timeout() {
        let (handle, server_task, addr) = start_server().await;

        let (mut client1, _conn1) = connect(addr).await;
        let (mut client2, _conn2) = connect(addr).await;

        // Clients can send request before graceful shutdown.
        do_empty_request(&mut client1).await.unwrap();
        do_empty_request(&mut client2).await.unwrap();

        let start = tokio::time::Instant::now();

        let (hdr1_tx, hdr1_rx) = oneshot::channel::<()>();

        let task1 = async {
            // A slow request made before graceful shutdown is handled.
            // This one is shorter than the timeout, so it should succeed.
            let hdr1 = send_slow_request(&mut client1, Duration::from_millis(222)).await;
            hdr1_tx.send(()).unwrap();

            let res1 = recv_slow_response_body(hdr1.unwrap()).await;
            res1.unwrap();
        };
        let task2 = async {
            // A slow request made before graceful shutdown is handled.
            // This one is much longer than the timeout; it should fail sometime
            // after the graceful shutdown timeout.

            let hdr2 = send_slow_request(&mut client2, Duration::from_millis(5_555)).await;
            hdr2.unwrap_err();
        };
        let task3 = async {
            // Begin graceful shutdown after we receive response headers for (1).
            hdr1_rx.await.unwrap();

            // Set a timeout on requests to finish before we drop them.
            handle.graceful_shutdown(Some(Duration::from_millis(333)));

            // Server task should finish soon.
            timeout(Duration::from_secs(1), server_task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();

            // At this point, graceful shutdown must have occured.
            assert!(start.elapsed() >= Duration::from_millis(222 + 333));
            assert!(start.elapsed() <= Duration::from_millis(5_555));
        };

        tokio::join!(task1, task2, task3);
    }

    async fn start_server() -> (
        Handle<IpSocketAddr>,
        JoinHandle<io::Result<()>>,
        IpSocketAddr,
    ) {
        let handle = Handle::new();

        let server_handle = handle.clone();
        let server_task = tokio::spawn(async move {
            let app = Router::new()
                .route("/", get(|| async { "Hello, world!" }))
                .route(
                    "/echo_slowly",
                    post(|body: Bytes| async move {
                        // Stream a response slowly, byte-by-byte, over 100ms
                        Response::new(slow_body(body.len(), Duration::from_millis(100)))
                    }),
                );

            let addr = IpSocketAddr::from(([127, 0, 0, 1], 0));

            Server::bind(addr)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await.unwrap();

        (handle, server_task, addr)
    }

    async fn connect(addr: IpSocketAddr) -> (SendRequest<Body>, JoinHandle<()>) {
        let stream = TokioIo::new(TcpStream::connect(addr).await.unwrap());
        let (send_request, connection) = handshake(stream).await.unwrap();

        let task = tokio::spawn(async move {
            let _ = connection.await;
        });

        (send_request, task)
    }

    // Send a basic `GET /` request.
    async fn do_empty_request(client: &mut SendRequest<Body>) -> hyper::Result<()> {
        client.ready().await?;

        let body = client
            .send_request(Request::new(Body::empty()))
            .await?
            .into_body();

        let body = body.collect().await?.to_bytes();
        assert_eq!(body.as_ref(), b"Hello, world!");
        Ok(())
    }

    // Send a request with a body streamed byte-by-byte, over a given duration,
    // then wait for the full response.
    async fn do_slow_request(
        client: &mut SendRequest<Body>,
        duration: Duration,
    ) -> hyper::Result<()> {
        let response = send_slow_request(client, duration).await?;
        recv_slow_response_body(response).await
    }

    async fn send_slow_request(
        client: &mut SendRequest<Body>,
        duration: Duration,
    ) -> hyper::Result<http::Response<hyper::body::Incoming>> {
        let req_body_len: usize = 10;
        let mut req = Request::new(slow_body(req_body_len, duration));
        *req.method_mut() = Method::POST;
        *req.uri_mut() = Uri::from_static("/echo_slowly");

        client.ready().await?;
        client.send_request(req).await
    }

    async fn recv_slow_response_body(
        response: http::Response<hyper::body::Incoming>,
    ) -> hyper::Result<()> {
        let resp_body = response.into_body();
        let resp_body_bytes = resp_body.collect().await?.to_bytes();
        assert_eq!(10, resp_body_bytes.len());
        Ok(())
    }

    // A stream of n response data `Frame`s, where n = `length`, and each frame
    // consists of a single byte. The whole response is smeared out over
    // a `duration` length of time.
    fn slow_body(length: usize, duration: Duration) -> axum::body::Body {
        let frames =
            (0..length).map(move |_| Ok::<_, hyper::Error>(Frame::data(Bytes::from_static(b"X"))));

        let stream = stream::iter(frames).then(move |frame| async move {
            tokio::time::sleep(duration / (length as u32)).await;
            frame
        });

        axum::body::Body::new(StreamBody::new(stream))
    }
}
