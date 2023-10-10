//! The PROXY protocol header compatibility.
//! See spec: <https://www.haproxy.org/download/1.8/doc/proxy-protocol.txt>
//!
//! Note: if you are setting a custom acceptor, `proxy_protocol_enabled` must be called after this is set.
//! Safest to use directly before serve when all options are configured.
//!
//! # Example
//!
//! ```rust,no_run
//! use axum::{routing::get, Router};
//! use std::net::SocketAddr;
//!
//! #[tokio::main]
//! async fn main() {
//!    let app = Router::new().route("/", get(|| async { "Hello, world!" }));
//!
//!    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
//!    println!("listening on {}", addr);
//!    axum_server::bind(addr)
//!        .proxy_protocol_enabled()
//!        .serve(app.into_make_service())
//!        .await
//!        .unwrap();
//! }
//! ```
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use http::HeaderValue;
use http::Request;
use ppp::{v1, v2, HeaderResult};
use tower_service::Service;

use crate::accept::Accept;
use std::fmt;
use tokio::io::AsyncReadExt;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod future;
use self::future::ProxyProtocolAcceptorFuture;

const V1_PREFIX_LEN: usize = 5;
const V2_PREFIX_LEN: usize = 12;
const V2_MINIMUM_LEN: usize = 16;
/// The index of the version-command byte.
const VERSION_COMMAND: usize = V2_PREFIX_LEN;
/// The index of the address family-protocol byte.
const ADDRESS_FAMILY_PROTOCOL: usize = VERSION_COMMAND + 1;
/// The index of the start of the big-endian u16 length.
const LENGTH: usize = ADDRESS_FAMILY_PROTOCOL + 1;
const DEFAULT_BUFFER_LEN: usize = 512;

/// Note: Currently only supports the V2 PROXY header format.
/// See proxy header spec: <https://www.haproxy.org/download/1.8/doc/proxy-protocol.txt>
pub(crate) async fn read_proxy_header<I>(mut stream: I) -> Result<(I, Option<IpAddr>), io::Error>
where
    I: AsyncRead + Unpin,
{
    // mutable buffer for storing stream data
    let mut buffer = [0; DEFAULT_BUFFER_LEN];

    // 1. read minimum length
    if let Err(e) = stream.read_exact(&mut buffer[..V2_PREFIX_LEN]).await {
        let msg = format!("Stream `read_exact` error: {}. Not able to read enough bytes to find a V2 Proxy Protocol header.", e);
        return Err(std::io::Error::new(e.kind(), msg));
    }

    // 2. check for v2 prefix
    if &buffer[..V2_PREFIX_LEN] != v2::PROTOCOL_PREFIX {
        if &buffer[..V1_PREFIX_LEN] == v1::PROTOCOL_PREFIX.as_bytes() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "V1 Proxy Protocol header detected, which is not supported",
            ));
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "No valid Proxy Protocol header detected",
            ));
        }
    }

    // 3. read the remaining header length
    let length = u16::from_be_bytes([buffer[LENGTH], buffer[LENGTH + 1]]) as usize;
    let full_length = V2_MINIMUM_LEN + length;

    if full_length > DEFAULT_BUFFER_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("V2 Proxy Protocol header length is too long. Consider increasing default buffer size. Length: {}", length),
        ));
    }

    if let Err(e) = stream.read_exact(&mut buffer[..full_length]).await {
        let msg = format!("Stream `read_exact` error: {}. Not able to read enough bytes to read the full V2 Proxy Protocol header.", e);
        return Err(std::io::Error::new(e.kind(), msg));
    }

    // 4. parse the header
    let header = HeaderResult::parse(&buffer[..full_length]);

    match header {
        HeaderResult::V1(Ok(_header)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "V1 Proxy Protocol header detected when parsing data",
            ));
        }
        HeaderResult::V2(Ok(header)) => {
            let client_address = match header.addresses {
                v2::Addresses::IPv4(ip) => IpAddr::V4(ip.source_address),
                v2::Addresses::IPv6(ip) => IpAddr::V6(ip.source_address),
                v2::Addresses::Unix(unix) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "Unix socket addresses are not supported. Addresses: {:?}",
                            unix
                        ),
                    ));
                }
                v2::Addresses::Unspecified => {
                    // V2 PROXY header addresses "Unspecified"
                    // Return client address as `None` so that "unknown" is used in the http header
                    return Ok((stream, None));
                }
            };

            Ok((stream, Some(client_address)))
        }
        HeaderResult::V1(Err(_error)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "No valid V1 Proxy Protocol header received",
            ));
        }
        HeaderResult::V2(Err(_error)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "No valid V2 Proxy Protocol header received",
            ));
        }
    }
}

/// Middleware for adding client IP address to the request `forwarded` header.
/// see spec: <https://www.rfc-editor.org/rfc/rfc7239#section-5.2>
#[derive(Debug)]
pub struct ForwardClientIp<S> {
    inner: S,
    client_address_opt: Option<IpAddr>,
}

impl<S> Service<Request<hyper::Body>> for ForwardClientIp<S>
where
    S: Service<Request<hyper::Body>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<hyper::Body>) -> Self::Future {
        let forwarded_string = match self.client_address_opt {
            Some(ip) => match ip {
                IpAddr::V4(ipv4) => {
                    format!("for={}", ipv4)
                }
                IpAddr::V6(ipv6) => {
                    format!("for=\"[{}]\"", ipv6)
                }
            },
            None => "for=unknown".to_string(),
        };

        if let Ok(header_value) = HeaderValue::from_str(&forwarded_string) {
            req.headers_mut().insert("Forwarded", header_value);
        }

        self.inner.call(req)
    }
}

#[derive(Clone)]
pub struct ProxyProtocolAcceptor<A> {
    inner: A,
    parsing_timeout: Duration,
}

impl<A> ProxyProtocolAcceptor<A> {
    pub(crate) fn new(inner: A) -> Self {
        #[cfg(not(test))]
        let parsing_timeout = Duration::from_secs(10);

        // Don't force tests to wait too long.
        #[cfg(test)]
        let parsing_timeout = Duration::from_secs(1);

        Self {
            inner,
            parsing_timeout,
        }
    }

    /// Override the default Proxy Header parsing timeout of 10 seconds, except during testing.
    pub fn parsing_timeout(mut self, val: Duration) -> Self {
        self.parsing_timeout = val;
        self
    }
}

impl<A> ProxyProtocolAcceptor<A> {
    /// Overwrite inner acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> ProxyProtocolAcceptor<Acceptor> {
        ProxyProtocolAcceptor {
            inner: acceptor,
            parsing_timeout: self.parsing_timeout,
        }
    }
}

impl<A, I, S> Accept<I, S> for ProxyProtocolAcceptor<A>
where
    A: Accept<I, S> + Clone,
    A::Stream: AsyncRead + AsyncWrite + Unpin,
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = A::Stream;
    type Service = ForwardClientIp<A::Service>;
    type Future = ProxyProtocolAcceptorFuture<
        Pin<Box<dyn Future<Output = Result<(I, Option<IpAddr>), io::Error>> + Send>>,
        A,
        I,
        S,
    >;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let read_header_future = Box::pin(read_proxy_header(stream));
        let inner_acceptor = self.inner.clone();
        let parsing_timeout = self.parsing_timeout;

        ProxyProtocolAcceptorFuture::new(
            read_header_future,
            inner_acceptor,
            service,
            parsing_timeout,
        )
    }
}

impl<A> fmt::Debug for ProxyProtocolAcceptor<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyProtocolAcceptor").finish()
    }
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
}
