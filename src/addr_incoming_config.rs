use std::time::Duration;

/// A configuration for [`AddrIncoming`](hyper::server::conn::AddrIncoming).
#[derive(Debug, Clone)]
pub struct AddrIncomingConfig {
    pub(crate) tcp_sleep_on_accept_errors: bool,
    pub(crate) tcp_keepalive: Option<Duration>,
    pub(crate) tcp_keepalive_interval: Option<Duration>,
    pub(crate) tcp_keepalive_retries: Option<u32>,
    pub(crate) tcp_nodelay: bool,
}

impl Default for AddrIncomingConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl AddrIncomingConfig {
    /// Creates a default [`AddrIncoming`](hyper::server::conn::AddrIncoming) config.
    pub fn new() -> AddrIncomingConfig {
        Self {
            tcp_sleep_on_accept_errors: true,
            tcp_keepalive: None,
            tcp_keepalive_interval: None,
            tcp_keepalive_retries: None,
            tcp_nodelay: false,
        }
    }

    /// Builds the config, creating an owned version of it.
    pub fn build(&mut self) -> Self {
        self.clone()
    }

    /// Set whether to sleep on accept errors, to avoid exhausting file descriptor limits.
    ///
    /// Default is `true`.
    pub fn tcp_sleep_on_accept_errors(&mut self, val: bool) -> &mut Self {
        self.tcp_sleep_on_accept_errors = val;
        self
    }

    /// Set how often to send TCP keepalive probes.
    ///
    /// By default TCP keepalive probes is disabled.
    pub fn tcp_keepalive(&mut self, val: Option<Duration>) -> &mut Self {
        self.tcp_keepalive = val;
        self
    }

    /// Set the duration between two successive TCP keepalive retransmissions,
    /// if acknowledgement to the previous keepalive transmission is not received.
    ///
    /// Default is no interval.
    pub fn tcp_keepalive_interval(&mut self, val: Option<Duration>) -> &mut Self {
        self.tcp_keepalive_interval = val;
        self
    }

    /// Set the number of retransmissions to be carried out before declaring that remote end is not available.
    ///
    /// Default is no retry.
    pub fn tcp_keepalive_retries(&mut self, val: Option<u32>) -> &mut Self {
        self.tcp_keepalive_retries = val;
        self
    }

    /// Set the value of `TCP_NODELAY` option for accepted connections.
    ///
    /// Default is `false`.
    pub fn tcp_nodelay(&mut self, val: bool) -> &mut Self {
        self.tcp_nodelay = val;
        self
    }
}
