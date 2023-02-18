use crate::notify_once::NotifyOnce;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{sync::Notify, time::sleep};

/// A handle for [`Server`](crate::server::Server).
#[derive(Clone, Debug, Default)]
pub struct Handle {
    inner: Arc<HandleInner>,
}

#[derive(Debug, Default)]
struct HandleInner {
    addr: Mutex<Option<SocketAddr>>,
    addr_notify: Notify,
    conn_count: AtomicUsize,
    shutdown: NotifyOnce,
    graceful: NotifyOnce,
    graceful_dur: Mutex<Option<Duration>>,
    conn_end: NotifyOnce,
}

impl Handle {
    /// Create a new handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the number of connections.
    pub fn connection_count(&self) -> usize {
        self.inner.conn_count.load(Ordering::SeqCst)
    }

    /// Shutdown the server.
    pub fn shutdown(&self) {
        self.inner.shutdown.notify_waiters();
    }

    /// Gracefully shutdown the server.
    ///
    /// `None` means indefinite grace period.
    pub fn graceful_shutdown(&self, duration: Option<Duration>) {
        *self.inner.graceful_dur.lock().unwrap() = duration;

        self.inner.graceful.notify_waiters();
    }

    /// Returns local address and port when server starts listening.
    ///
    /// Returns `None` if server fails to bind.
    pub async fn listening(&self) -> Option<SocketAddr> {
        let notified = self.inner.addr_notify.notified();

        if let Some(addr) = *self.inner.addr.lock().unwrap() {
            return Some(addr);
        }

        notified.await;

        *self.inner.addr.lock().unwrap()
    }

    pub(crate) fn notify_listening(&self, addr: Option<SocketAddr>) {
        *self.inner.addr.lock().unwrap() = addr;

        self.inner.addr_notify.notify_waiters();
    }

    pub(crate) fn watcher(&self) -> Watcher {
        Watcher::new(self.clone())
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
}

pub(crate) struct Watcher {
    handle: Handle,
}

impl Watcher {
    fn new(handle: Handle) -> Self {
        handle.inner.conn_count.fetch_add(1, Ordering::SeqCst);

        Self { handle }
    }

    pub(crate) async fn wait_graceful_shutdown(&self) {
        self.handle.wait_graceful_shutdown().await
    }

    pub(crate) async fn wait_shutdown(&self) {
        self.handle.wait_shutdown().await
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let count = self.handle.inner.conn_count.fetch_sub(1, Ordering::SeqCst) - 1;

        if count == 0 && self.handle.inner.graceful.is_notified() {
            self.handle.inner.conn_end.notify_waiters();
        }
    }
}
