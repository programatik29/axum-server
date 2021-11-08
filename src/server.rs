use crate::service::MakeServiceRef;
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
    sync::{Arc, Mutex},
};
use tokio::{net::TcpListener, sync::Notify};

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
        let listener = TcpListener::bind(self.addr).await?;
        let mut incoming = AddrIncoming::from_listener(listener).map_err(io_other)?;

        self.handle.notify_listening(incoming.local_addr());

        loop {
            let addr_stream = accept(&mut incoming).await?;

            poll_fn(|cx| make_service.poll_ready(cx))
                .await
                .map_err(io_other)?;

            let service = match make_service.make_service(&addr_stream).await {
                Ok(service) => service,
                Err(_) => continue,
            };

            tokio::spawn(async move {
                let _ = Http::new()
                    .serve_connection(addr_stream, service)
                    .with_upgrades()
                    .await;
            });
        }
    }
}

async fn accept(incoming: &mut AddrIncoming) -> io::Result<AddrStream> {
    let mut incoming = Pin::new(incoming);

    // Always [`Option::Some`].
    // https://docs.rs/hyper/0.14.14/src/hyper/server/tcp.rs.html#165
    poll_fn(|cx| incoming.as_mut().poll_accept(cx))
        .await
        .unwrap()
}

/// A handle for [`Server`].
#[derive(Clone, Debug, Default)]
pub struct Handle {
    inner: Arc<HandleInner>,
}

#[derive(Debug, Default)]
struct HandleInner {
    addr: Mutex<Option<SocketAddr>>,
    addr_notify: Notify,
}

impl Handle {
    /// Create a new handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns local address and port when server starts listening.
    pub async fn listening(&self) -> SocketAddr {
        let notified = self.inner.addr_notify.notified();

        match *self.inner.addr.lock().unwrap() {
            Some(addr) => return addr,
            None => (),
        }

        notified.await;

        self.inner
            .addr
            .lock()
            .unwrap()
            .expect("notified before address is set")
    }

    fn notify_listening(&self, addr: SocketAddr) {
        *self.inner.addr.lock().unwrap() = Some(addr);

        self.inner.addr_notify.notify_waiters();
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn io_other<E: Into<BoxError>>(error: E) -> io::Error {
    io::Error::new(ErrorKind::Other, error)
}

#[cfg(test)]
mod tests {
    use crate::{Handle, Server};
    use axum::{routing::get, Router};
    use hyper::client::Client;
    use std::net::SocketAddr;

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
                .unwrap();
        });

        let addr = handle.listening().await;

        let client = Client::new();
        let response = client.get(format!("http://{}/", addr).parse().unwrap()).await.unwrap();
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

        assert_eq!(body.as_ref(), b"Hello, world!");
    }
}
