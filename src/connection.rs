//! Connection-level control.

use tokio::sync::watch;

/// How a connection was asked to close.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ShutdownKind {
    #[default]
    None,
    Graceful,
    Abort,
}

/// A handle to the connection that dispatched the current request.
///
/// axum-server adds this to the extensions of every request, so a handler can
/// close the connection it is running on:
///
/// ```rust,no_run
/// use axum::{extract::Extension, routing::get, Router};
/// use axum_server::ConnectionShutdown;
///
/// async fn handle(Extension(shutdown): Extension<ConnectionShutdown>) -> &'static str {
///     shutdown.graceful_shutdown();
///     "goodbye"
/// }
///
/// let app: Router = Router::new().route("/", get(handle));
/// ```
///
/// The handle can be moved into another task to close the connection later,
/// for example to enforce a deadline on a slow client.
///
/// An HTTP/2 connection carries many streams, so closing it closes all of
/// them.
#[derive(Clone, Debug)]
pub struct ConnectionShutdown {
    tx: watch::Sender<ShutdownKind>,
}

impl ConnectionShutdown {
    pub(crate) fn new(tx: watch::Sender<ShutdownKind>) -> Self {
        Self { tx }
    }

    /// Close the connection immediately.
    ///
    /// The in-flight request is cancelled, so its response may not be sent.
    /// Use [`graceful_shutdown`](Self::graceful_shutdown) to let the response
    /// finish.
    pub fn shutdown(&self) {
        let _ = self.tx.send_replace(ShutdownKind::Abort);
    }

    /// Close the connection after the in-flight response is sent.
    ///
    /// The current response is still delivered and no further requests are
    /// accepted on the connection.
    pub fn graceful_shutdown(&self) {
        let _ = self.tx.send_if_modified(|kind| match kind {
            ShutdownKind::None => {
                *kind = ShutdownKind::Graceful;
                true
            }
            ShutdownKind::Graceful | ShutdownKind::Abort => false,
        });
    }
}
