//! Information about TLS connections.
//!
//! After the TLS handshake completes, the TLS acceptors add a [`TlsInfo`]
//! value to the extensions of every request on that connection. Handlers can
//! retrieve it as `Arc<TlsInfo>`; with `axum`, use
//! `Extension<Arc<TlsInfo>>`.

use http::Request;
use hyper::body::Incoming;
use std::{fmt, sync::Arc};
use tower_service::Service;

/// Information about an established TLS connection.
///
/// This is inserted into request extensions as `Arc<TlsInfo>`.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct TlsInfo {
    /// The server name the client requested using SNI, if any.
    pub server_name: Option<String>,
    /// The protocol negotiated using ALPN, for example `h2` or `http/1.1`.
    pub alpn_protocol: Option<Vec<u8>>,
    /// DER-encoded certificates presented by the client, if any.
    pub peer_certificates: Vec<Vec<u8>>,
}

impl TlsInfo {
    pub(crate) fn new(
        server_name: Option<String>,
        alpn_protocol: Option<Vec<u8>>,
        peer_certificates: Vec<Vec<u8>>,
    ) -> Self {
        Self {
            server_name,
            alpn_protocol,
            peer_certificates,
        }
    }
}

/// A service that adds [`TlsInfo`] to request extensions.
#[derive(Clone)]
pub struct TlsService<S> {
    inner: S,
    info: Arc<TlsInfo>,
}

impl<S> fmt::Debug for TlsService<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsService")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl<S> TlsService<S> {
    pub(crate) fn new(inner: S, info: TlsInfo) -> Self {
        Self {
            inner,
            info: Arc::new(info),
        }
    }
}

impl<S> Service<Request<Incoming>> for TlsService<S>
where
    S: Service<Request<Incoming>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Incoming>) -> Self::Future {
        req.extensions_mut().insert(self.info.clone());

        self.inner.call(req)
    }
}
