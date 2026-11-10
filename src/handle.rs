use crate::{notify_once::NotifyOnce, server::Address};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::sleep;

/// A handle for shutdown and connection counting that is not tied to an
/// address type.
///
/// Unlike [`Handle`], one `Control` can drive servers listening on different
/// address types, for example TCP and Unix at the same time. Build a [`Handle`]
/// for each listener with [`Handle::with_control`], then
/// [`graceful_shutdown`](Self::graceful_shutdown) or [`shutdown`](Self::shutdown)
/// drains all of them and [`connection_count`](Self::connection_count) covers
/// every listener.
///
/// ```rust
/// use axum_server::{Control, Handle};
/// use std::net::SocketAddr;
///
/// let control = Control::new();
///
/// let handle = Handle::<SocketAddr>::with_control(control.clone());
/// ```
#[derive(Clone, Debug, Default)]
pub struct Control {
    inner: Arc<ControlInner>,
}

#[derive(Debug, Default)]
struct ControlInner {
    conn_count: AtomicUsize,
    shutdown: NotifyOnce,
    graceful: NotifyOnce,
    graceful_dur: Mutex<Option<Duration>>,
    conn_end: NotifyOnce,
}

impl Control {
    /// Create a new control handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the number of live connections across all servers using this
    /// control.
    pub fn connection_count(&self) -> usize {
        self.inner.conn_count.load(Ordering::SeqCst)
    }

    /// Shutdown all servers using this control.
    pub fn shutdown(&self) {
        self.inner.shutdown.notify_waiters();
    }

    /// Gracefully shutdown all servers using this control.
    ///
    /// `None` means indefinite grace period.
    pub fn graceful_shutdown(&self, duration: Option<Duration>) {
        *self.inner.graceful_dur.lock().unwrap() = duration;

        self.inner.graceful.notify_waiters();
    }

    pub(crate) async fn wait_shutdown(&self) {
        self.inner.shutdown.notified().await;
    }

    pub(crate) async fn wait_graceful_shutdown(&self) {
        self.inner.graceful.notified().await;
    }

    pub(crate) async fn wait_connections_end(&self) {
        if self.inner.conn_count.load(Ordering::SeqCst) == 0 {
            return;
        }

        let deadline = *self.inner.graceful_dur.lock().unwrap();

        match deadline {
            Some(duration) => tokio::select! {
                biased;
                _ = sleep(duration) => self.shutdown(),
                _ = self.inner.conn_end.notified() => (),
            },
            None => self.inner.conn_end.notified().await,
        }
    }

    pub(crate) fn watcher(&self) -> Watcher {
        Watcher::new(self.clone())
    }
}

/// A handle for [`Server`](crate::server::Server).
///
/// It controls the server it is passed to and, through
/// [`with_control`](Self::with_control), any number of servers sharing the same
/// [`Control`].
#[derive(Clone, Debug)]
pub struct Handle<A: Address> {
    addr: Arc<AddrSlot<A>>,
    control: Control,
}

#[derive(Debug)]
struct AddrSlot<A: Address> {
    addr: Mutex<Option<A>>,
    addr_notify: NotifyOnce,
}

// Manually implemented as the derive macro will want A to be Default.
impl<A: Address> Default for AddrSlot<A> {
    fn default() -> Self {
        Self {
            addr: Default::default(),
            addr_notify: Default::default(),
        }
    }
}

impl<A: Address> Default for Handle<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Address> Handle<A> {
    /// Create a new handle.
    pub fn new() -> Self {
        Self::with_control(Control::new())
    }

    /// Create a handle that shares shutdown and connection counting with
    /// `control`.
    ///
    /// Each server should get its own `Handle` so that
    /// [`listening`](Self::listening) reports the address of that server.
    pub fn with_control(control: Control) -> Self {
        Self {
            addr: Arc::new(AddrSlot::default()),
            control,
        }
    }

    /// Get the shared [`Control`].
    pub fn control(&self) -> Control {
        self.control.clone()
    }

    /// Get the number of connections.
    pub fn connection_count(&self) -> usize {
        self.control.connection_count()
    }

    /// Shutdown the server.
    pub fn shutdown(&self) {
        self.control.shutdown();
    }

    /// Gracefully shutdown the server.
    ///
    /// `None` means indefinite grace period.
    pub fn graceful_shutdown(&self, duration: Option<Duration>) {
        self.control.graceful_shutdown(duration);
    }

    /// Returns local address and port when server starts listening.
    ///
    /// Returns `None` if server fails to bind.
    pub async fn listening(&self) -> Option<A> {
        // Wait for the server to resolve its listening address. This is a
        // one-shot latch: if binding already failed, this returns immediately
        // with `None` instead of waiting forever for a notification that will
        // never come.
        self.addr.addr_notify.notified().await;

        self.addr.addr.lock().unwrap().clone()
    }

    pub(crate) fn notify_listening(&self, addr: Option<A>) {
        *self.addr.addr.lock().unwrap() = addr;

        self.addr.addr_notify.notify_waiters();
    }

    pub(crate) fn watcher(&self) -> Watcher {
        self.control.watcher()
    }

    pub(crate) async fn wait_shutdown(&self) {
        self.control.wait_shutdown().await;
    }

    pub(crate) async fn wait_graceful_shutdown(&self) {
        self.control.wait_graceful_shutdown().await;
    }

    pub(crate) async fn wait_connections_end(&self) {
        self.control.wait_connections_end().await;
    }
}

pub(crate) struct Watcher {
    control: Control,
}

impl Watcher {
    fn new(control: Control) -> Self {
        control.inner.conn_count.fetch_add(1, Ordering::SeqCst);

        Self { control }
    }

    pub(crate) async fn wait_graceful_shutdown(&self) {
        self.control.wait_graceful_shutdown().await
    }

    pub(crate) async fn wait_shutdown(&self) {
        self.control.wait_shutdown().await
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let count = self.control.inner.conn_count.fetch_sub(1, Ordering::SeqCst) - 1;

        if count == 0 && self.control.inner.graceful.is_notified() {
            self.control.inner.conn_end.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Control, Handle};
    use std::{net::SocketAddr, time::Duration};

    #[tokio::test]
    async fn listening_returns_none_after_bind_failure() {
        let handle = Handle::<SocketAddr>::new();

        // This is what `Server::serve` calls when binding fails.
        handle.notify_listening(None);

        let addr = tokio::time::timeout(Duration::from_secs(1), handle.listening())
            .await
            .expect("listening() should resolve immediately after a bind failure");

        assert_eq!(addr, None);
    }

    #[tokio::test]
    async fn listening_returns_addr_after_successful_bind() {
        let handle = Handle::<SocketAddr>::new();
        let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

        handle.notify_listening(Some(addr));

        let resolved = tokio::time::timeout(Duration::from_secs(1), handle.listening())
            .await
            .expect("listening() should resolve immediately");

        assert_eq!(resolved, Some(addr));
    }

    #[tokio::test]
    async fn control_is_shared_between_handles() {
        let control = Control::new();
        let first = Handle::<SocketAddr>::with_control(control.clone());
        let second = Handle::<SocketAddr>::with_control(control.clone());

        assert!(std::sync::Arc::ptr_eq(
            &first.control.inner,
            &second.control.inner
        ));
        assert!(std::sync::Arc::ptr_eq(&control.inner, &first.control.inner));
    }
}
