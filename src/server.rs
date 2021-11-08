use crate::{handle::Handle, service::MakeServiceRef};
use futures_util::future::poll_fn;
use http::Request;
use hyper::server::{
    accept::Accept,
    conn::{AddrIncoming, AddrStream, Http},
};
use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    pin::Pin,
};
use tokio::net::TcpListener;

/// HTTP server.
#[derive(Debug)]
pub struct Server {
    addr: SocketAddr,
    handle: Handle,
}

/// Create a [`Server`] that will bind to provided address.
pub fn bind(addr: SocketAddr) -> Server {
    Server::bind(addr)
}

impl Server {
    /// Create a server that will bind to provided address.
    pub fn bind(addr: SocketAddr) -> Self {
        let handle = Handle::new();

        Self { addr, handle }
    }

    /// Provide a handle for additional utilities.
    pub fn handle(mut self, handle: Handle) -> Self {
        self.handle = handle;
        self
    }

    /// Serve provided [`MakeService`].
    ///
    /// [`MakeService`]: https://docs.rs/tower/0.4/tower/make/trait.MakeService.html
    pub async fn serve<M>(self, mut make_service: M) -> io::Result<()>
    where
        M: MakeServiceRef<AddrStream, Request<hyper::Body>>,
    {
        let handle = self.handle;

        let listener = TcpListener::bind(self.addr).await?;
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

                let watcher = handle.watcher();

                tokio::spawn(async move {
                    let serve_future = Http::new()
                        .serve_connection(addr_stream, service)
                        .with_upgrades();

                    tokio::select! {
                        biased;
                        _ = watcher.wait_shutdown() => (),
                        _ = serve_future => (),
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
    use http::Request;
    use hyper::{
        client::conn::{handshake, SendRequest},
        Body,
    };
    use std::{net::SocketAddr, time::Duration};
    use tokio::{net::TcpStream, task::JoinHandle, time::timeout};
    use tower::{Service, ServiceExt};

    #[tokio::test]
    async fn start_and_request() {
        let handle = Handle::new();

        let server_handle = handle.clone();
        tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            Server::bind(addr)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await;

        let (client, _conn) = connect(addr).await;
        let response = client.oneshot(Request::new(Body::empty())).await.unwrap();
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

        assert_eq!(body.as_ref(), b"Hello, world!");
    }

    #[tokio::test]
    async fn test_shutdown() {
        let handle = Handle::new();

        let server_handle = handle.clone();
        tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            Server::bind(addr)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await;

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

        let addr = handle.listening().await;

        let (mut client, conn) = connect(addr).await;

        handle.graceful_shutdown(None);

        let response = client
            .ready()
            .await
            .unwrap()
            .call(Request::new(Body::empty()))
            .await
            .unwrap();
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

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

        let addr = handle.listening().await;

        let (mut client, _conn) = connect(addr).await;

        handle.graceful_shutdown(Some(Duration::from_millis(250)));

        let response = client
            .ready()
            .await
            .unwrap()
            .call(Request::new(Body::empty()))
            .await
            .unwrap();
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

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

    async fn connect(addr: SocketAddr) -> (SendRequest<Body>, JoinHandle<()>) {
        let stream = TcpStream::connect(addr).await.unwrap();

        let (send_request, connection) = handshake(stream).await.unwrap();

        let task = tokio::spawn(async move {
            let _ = connection.await;
        });

        (send_request, task)
    }
}
