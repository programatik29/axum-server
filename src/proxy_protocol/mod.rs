//! The PROXY protocol header compatibility.
//! See spec: <https://www.haproxy.org/download/1.8/doc/proxy-protocol.txt>
use std::fmt::Debug;
use std::io;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::task::{Context, Poll};

use futures::future::poll_fn;
use http::HeaderValue;
use http::Request;
use hyper::server::conn::AddrStream;
use ppp::{v2, HeaderResult, PartialResult};
use tokio::io::AsyncReadExt;
use tokio::io::ReadBuf;
use tokio::time::{timeout, Duration};
use tower_service::Service;

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
pub(crate) async fn read_proxy_header(stream: &mut AddrStream) -> Result<IpAddr, io::Error> {
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
    const TIMEOUT_DURATION: Duration = Duration::from_secs(1);
    timeout(TIMEOUT_DURATION, async {
        loop {
            // Peek the stream data into the buffer.
            // Each loop re-reads the stream from beginning,
            // but further in if more stream has arrived.
            poll_fn(|cx| {
                stream.poll_peek(cx, &mut buffer)
            }).await?;

            let header = HeaderResult::parse(buffer.filled());

            if header.is_complete() {
                break Ok(());
            } else if buffer.remaining() == 0 {
                return Err(io::Error::new(io::ErrorKind::Other, "Buffer limit reached without finding a complete header. Consider increasing the buffer size."));
            }

            buffer.clear();
        }
    }).await??;

    let header = HeaderResult::parse(buffer.filled());

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
#[derive(Clone, Copy, Debug)]
pub(crate) struct ForwardClientAddress<S> {
    pub(crate) inner: S,
    pub(crate) address: Option<IpAddr>,
}

impl<B, S> Service<Request<B>> for ForwardClientAddress<S>
where
    S: Service<Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let forwarded_string = match self.address {
            Some(address) => match address {
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
