//! The PROXY protocol header compatibility.
//! See spec: <https://www.haproxy.org/download/1.8/doc/proxy-protocol.txt>
//!
//! Note: if you are setting a custom acceptor, `proxy_protocol_enabled` must be called afterwards.
//! Safest to use directly before serve when all options are configured.
//!
//! # Example
//!
//! ```rust,no_run
//! use axum::{routing::get, Router};
//! use axum_server::tls_rustls::RustlsConfig;
//! use std::net::SocketAddr;
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = Router::new().route("/", get(|| async { "Hello, world!" }));
//!
//!     let config = RustlsConfig::from_pem_file(
//!         "examples/self-signed-certs/cert.pem",
//!         "examples/self-signed-certs/key.pem",
//!     )
//!     .await
//!     .unwrap();
//!
//!     let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
//!     println!("listening on {}", addr);
//!     axum_server::bind_rustls(addr, config)
//!         .proxy_protocol_enabled()
//!         .serve(app.into_make_service())
//!         .await
//!         .unwrap();
//! }
//! ```
use std::future::poll_fn;
use std::io;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use std::future::Future;
use std::pin::Pin;

use http::HeaderValue;
use http::Request;
use ppp::{v2, HeaderResult, PartialResult};
use tokio::io::AsyncReadExt;
use tokio::io::ReadBuf;
use tower_service::Service;

use crate::accept::Accept;
use std::fmt;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod future;
use self::future::ProxyProtocolAcceptorFuture;

pub trait Peekable {
    fn poll_peek(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<usize>>;
}

/// Parse and remove the PROXY protocol header from the front of the stream if it exists.
///
/// Attempts to read the PROXY header from the given `AddrStream`. If a valid header is detected,
/// it is removed from the stream and the client's address (if present) is returned.
///
/// # Arguments:
///
/// * `stream`: The mutable reference to a `AddrStream` from which the PROXY header should be read.
///
/// # Returns:
///
/// A `Result<IpAddr, io::Error>` where:
/// * `Ok(IpAddr)` indicates successful parsing of a valid PROXY header and contains the client's address.
/// * `Err(io::Error)` indicates an error occurred during the parsing process. The error provides more details
///   about the nature of the issue, such as timeout, invalid header format, unsupported version, etc.
///
/// Note: Currently only supports the V2 PROXY header format.
/// See proxy header spec: <https://www.haproxy.org/download/1.8/doc/proxy-protocol.txt>
pub(crate) async fn read_proxy_header<I>(stream: &mut I) -> Result<IpAddr, io::Error>
where
    I: Peekable + AsyncRead + Unpin,
{
    // Maximum expected size for PROXY header is
    // unlikely to be larger than 512 bytes unless additional fields are added.
    // Maximum length if no additional fields is
    //      12 (signature)
    //    + 1 (version & command)
    //    + 1 (Protocol & address family)
    //    + 2 (length - combined length of address, port, and additional fields)
    //    + 216 (UNIX)
    //    = 232 bytes
    // For IPv6 maximum is 12 + 1 + 1 + 2 + 54 = 70 bytes
    const BUFFER_SIZE: usize = 512;
    // Mutable buffer for storing the bytes received from the stream.
    let mut array_buffer = [0; BUFFER_SIZE];
    let mut buffer = ReadBuf::new(&mut array_buffer);

    // peek the stream until find a complete header
    let header = loop {
        // Peek the stream data into the buffer.
        // Each loop re-reads the stream from beginning,
        // but further in if more stream has arrived.
        poll_fn(|cx| stream.poll_peek(cx, &mut buffer)).await?;

        let header = HeaderResult::parse(buffer.filled());

        if header.is_complete() {
            break Ok(header);
        } else if buffer.remaining() == 0 {
            break Err(io::Error::new(io::ErrorKind::Other, "Buffer limit reached without finding a complete header. Consider increasing the buffer size."));
        }

        buffer.clear();
    }?;

    match header {
        HeaderResult::V1(Ok(header)) => Err(io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "V1 PROXY header detected, which is not supported. Header: {:?}",
                header
            ),
        )),
        HeaderResult::V2(Ok(header)) => {
            let client_address = match header.addresses {
                v2::Addresses::IPv4(ip) => IpAddr::V4(ip.source_address),
                v2::Addresses::IPv6(ip) => IpAddr::V6(ip.source_address),
                v2::Addresses::Unix(unix) => {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "Unix socket addresses are not supported. Addresses: {:?}",
                            unix
                        ),
                    ));
                }
                v2::Addresses::Unspecified => {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "V2 PROXY header addresses \"Unspecified\"",
                    ));
                }
            };

            // Remove header length in bytes from the stream
            let header_len = header.len();
            stream
                .read_exact(&mut buffer.filled_mut()[..header_len])
                .await?;

            Ok(client_address)
        }
        HeaderResult::V1(Err(error)) => Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("No valid V1 PROXY header received: {}", error),
        )),
        HeaderResult::V2(Err(error)) => Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("No valid V2 PROXY header received: {}", error),
        )),
    }
}

/// Middleware for adding client IP address to the request `forwarded` header.
/// see spec: <https://www.rfc-editor.org/rfc/rfc7239#section-5.2>
pub(crate) struct ForwardClientIp<S> {
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

pub struct ProxyProtocolAcceptor<A> {
    inner: A,
    job_timeout: Duration,
}

impl<A> ProxyProtocolAcceptor<A> {
    pub fn new(inner: A) -> Self {
        #[cfg(not(test))]
        let job_timeout = Duration::from_secs(10);

        // Don't force tests to wait too long.
        #[cfg(test)]
        let job_timeout = Duration::from_secs(1);

        Self {
            inner,
            job_timeout,
        }
    }

    /// Override the default Proxy Header parsing timeout of 10 seconds, except during testing.
    pub fn job_timeout(mut self, val: Duration) -> Self {
        self.job_timeout = val;
        self
    }
}

impl<A> ProxyProtocolAcceptor<A> {
    /// Overwrite inner acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> ProxyProtocolAcceptor<Acceptor> {
        ProxyProtocolAcceptor {
            inner: acceptor,
            job_timeout: self.job_timeout,
        }
    }
}

impl<A, I, S> Accept<I, S> for ProxyProtocolAcceptor<A>
where
    A: Accept<I, S>,
    A::Stream: AsyncRead + AsyncWrite + Unpin,
    I: AsyncRead + AsyncWrite + Unpin + Peekable + Send + 'static,
{
    type Stream = A::Stream;
    type Service = ForwardClientIp<A::Service>;
    type Future = ProxyProtocolAcceptorFuture<Pin<Box<dyn Future<Output = Result<IpAddr, io::Error>> + Send>>, A, I, S>;

    fn accept(&self, mut stream: I, service: S) -> Self::Future {
        // TODO: wrap in timeout

        let read_header_future = Box::pin(read_proxy_header(&mut stream));

        ProxyProtocolAcceptorFuture::new(read_header_future, self.inner, stream, service)
    }
}

impl<A> fmt::Debug for ProxyProtocolAcceptor<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyProtocolAcceptor").finish()
    }
}
