use std::time::Duration;

/// A configuration for [`AddrIncoming`].
#[derive(Debug, Clone)]
pub struct AddrIncomingConfig {
    pub(crate) tcp_sleep_on_accept_errors: bool,
    pub(crate) tcp_keepalive: Option<Duration>,
    pub(crate) tcp_nodelay: bool,
}

impl Default for AddrIncomingConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl AddrIncomingConfig {
    /// Creates a default [`AddrIncoming`] config.
    pub fn new() -> AddrIncomingConfig {
        Self {
            tcp_sleep_on_accept_errors: true,
            tcp_keepalive: None,
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
    /// Default is `false`.
    pub fn tcp_keepalive(&mut self, val: Option<Duration>) -> &mut Self {
        self.tcp_keepalive = val;
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
