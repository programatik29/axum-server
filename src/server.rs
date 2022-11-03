use crate::{
    accept::{Accept, DefaultAcceptor},
    addr_incoming_config::AddrIncomingConfig,
    handle::Handle,
    http_config::HttpConfig,
    service::{MakeServiceRef, SendService},
};
use futures_util::future::poll_fn;
use http::Request;
use hyper::server::{
    accept::Accept as HyperAccept,
    conn::{AddrIncoming, AddrStream},
};
use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    pin::Pin,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
};

/// HTTP server.
#[derive(Debug)]
pub struct Server<A = DefaultAcceptor> {
    acceptor: A,
    listener: Listener,
    addr_incoming_conf: AddrIncomingConfig,
    handle: Handle,
    http_conf: HttpConfig,
}

#[derive(Debug)]
enum Listener {
    Bind(SocketAddr),
    Std(std::net::TcpListener),
}

/// Create a [`Server`] that will bind to provided address.
pub fn bind(addr: SocketAddr) -> Server {
    Server::bind(addr)
}

/// Create a [`Server`] from existing `std::net::TcpListener`.
pub fn from_tcp(listener: std::net::TcpListener) -> Server {
    Server::from_tcp(listener)
}

impl Server {
    /// Create a server that will bind to provided address.
    pub fn bind(addr: SocketAddr) -> Self {
        let acceptor = DefaultAcceptor::new();
        let handle = Handle::new();

        Self {
            acceptor,
            listener: Listener::Bind(addr),
            addr_incoming_conf: AddrIncomingConfig::default(),
            handle,
            http_conf: HttpConfig::default(),
        }
    }

    /// Create a server from existing `std::net::TcpListener`.
    pub fn from_tcp(listener: std::net::TcpListener) -> Self {
        let acceptor = DefaultAcceptor::new();
        let handle = Handle::new();

        Self {
            acceptor,
            listener: Listener::Std(listener),
            addr_incoming_conf: AddrIncomingConfig::default(),
            handle,
            http_conf: HttpConfig::default(),
        }
    }
}

impl<A> Server<A> {
    /// Overwrite acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> Server<Acceptor> {
        Server {
            acceptor,
            listener: self.listener,
            addr_incoming_conf: self.addr_incoming_conf,
            handle: self.handle,
            http_conf: self.http_conf,
        }
    }

    /// Map acceptor.
    pub fn map<Acceptor, F>(self, acceptor: F) -> Server<Acceptor>
    where
        F: FnOnce(A) -> Acceptor,
    {
        Server {
            acceptor: acceptor(self.acceptor),
            listener: self.listener,
            addr_incoming_conf: self.addr_incoming_conf,
            handle: self.handle,
            http_conf: self.http_conf,
        }
    }

    /// Returns a reference to the acceptor.
    pub fn get_ref(&self) -> &A {
        &self.acceptor
    }

    /// Returns a mutable reference to the acceptor.
    pub fn get_mut(&mut self) -> &mut A {
        &mut self.acceptor
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

    /// Overwrite addr incoming configuration.
    pub fn addr_incoming_config(mut self, config: AddrIncomingConfig) -> Self {
        self.addr_incoming_conf = config;
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
    /// [`axum`] make services.
    ///
    /// [`axum`]: https://docs.rs/axum/0.3
    /// [`tower`]: https://docs.rs/tower
    /// [`MakeService`]: https://docs.rs/tower/0.4/tower/make/trait.MakeService.html
    pub async fn serve<M>(self, mut make_service: M) -> io::Result<()>
    where
        M: MakeServiceRef<AddrStream, Request<hyper::Body>>,
        A: Accept<AddrStream, M::Service> + Clone + Send + Sync + 'static,
        A::Stream: AsyncRead + AsyncWrite + Unpin + Send,
        A::Service: SendService<Request<hyper::Body>> + Send,
        A::Future: Send,
    {
        let acceptor = self.acceptor;
        let addr_incoming_conf = self.addr_incoming_conf;
        let handle = self.handle;
        let http_conf = self.http_conf;

        let mut incoming = match bind_incoming(self.listener, addr_incoming_conf).await {
            Ok(v) => v,
            Err(e) => {
                handle.notify_listening(None);
                return Err(e);
            }
        };

        handle.notify_listening(Some(incoming.local_addr()));

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
                    if let Ok((stream, send_service)) = acceptor.accept(addr_stream, service).await
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
    }
}

async fn bind_incoming(
    listener: Listener,
    addr_incoming_conf: AddrIncomingConfig,
) -> io::Result<AddrIncoming> {
    let listener = match listener {
        Listener::Bind(addr) => TcpListener::bind(addr).await?,
        Listener::Std(std_listener) => {
            std_listener.set_nonblocking(true)?;
            TcpListener::from_std(std_listener)?
        }
    };
    let mut incoming = AddrIncoming::from_listener(listener).map_err(io_other)?;

    incoming.set_sleep_on_errors(addr_incoming_conf.tcp_sleep_on_accept_errors);
    incoming.set_keepalive(addr_incoming_conf.tcp_keepalive);
    incoming.set_keepalive_interval(addr_incoming_conf.tcp_keepalive_interval);
    incoming.set_keepalive_retries(addr_incoming_conf.tcp_keepalive_retries);
    incoming.set_nodelay(addr_incoming_conf.tcp_nodelay);

    Ok(incoming)
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
    use crate::{handle::Handle, server::Server};
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

            Server::bind(addr)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await.unwrap();

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
