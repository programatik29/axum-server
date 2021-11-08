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
        let future = self.notify.notified();

        if !self.notified.load(Ordering::SeqCst) {
            future.await;
        }
    }
}
