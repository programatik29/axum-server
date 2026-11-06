use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

#[derive(Debug, Default)]
pub(crate) struct NotifyOnce {
    notified: AtomicBool,
    notify: Notify,
}

impl NotifyOnce {
    pub(crate) fn notify_waiters(&self) {
        self.notified.store(true, Ordering::SeqCst);

        self.notify.notify_waiters();
    }

    pub(crate) fn is_notified(&self) -> bool {
        self.notified.load(Ordering::SeqCst)
    }

    pub(crate) async fn notified(&self) {
        // `Notify::notified` does not register a waiter until it is polled for
        // the first time. Enable it up front so a notification that arrives
        // between the flag check and the first poll is not lost.
        let mut notified = std::pin::pin!(self.notify.notified());
        notified.as_mut().enable();

        if !self.notified.load(Ordering::SeqCst) {
            notified.await;
        }
    }
}
