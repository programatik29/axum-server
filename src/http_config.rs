use hyper::server::conn::Http;
use std::time::Duration;

/// A configuration for [`Http`].
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub(crate) inner: Http,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpConfig {
    /// Creates a default [`Http`] config.
    pub fn new() -> HttpConfig {
        Self { inner: Http::new() }
    }

    /// Builds the config, creating an owned version of it.
    pub fn build(&mut self) -> Self {
        self.clone()
    }

    /// Sets whether HTTP1 is required.
    ///
    /// Default is `false`.
    pub fn http1_only(&mut self, val: bool) -> &mut Self {
        self.inner.http1_only(val);
        self
    }

    /// Set whether HTTP/1 connections should support half-closures.
    ///
    /// Clients can chose to shutdown their write-side while waiting
    /// for the server to respond. Setting this to `true` will
    /// prevent closing the connection immediately if `read`
    /// detects an EOF in the middle of a request.
    ///
    /// Default is `false`.
    pub fn http1_half_close(&mut self, val: bool) -> &mut Self {
        self.inner.http1_half_close(val);
        self
    }

    /// Enables or disables HTTP/1 keep-alive.
    ///
    /// Default is true.
    pub fn http1_keep_alive(&mut self, val: bool) -> &mut Self {
        self.inner.http1_keep_alive(val);
        self
    }

    /// Set whether HTTP/1 connections will write header names as title case at
    /// the socket level.
    ///
    /// Note that this setting does not affect HTTP/2.
    ///
    /// Default is false.
    pub fn http1_title_case_headers(&mut self, enabled: bool) -> &mut Self {
        self.inner.http1_title_case_headers(enabled);
        self
    }

    /// Set whether HTTP/1 connections will write header names as provided
    /// at the socket level.
    ///
    /// Note that this setting does not affect HTTP/2.
    ///
    /// Default is false.
    pub fn http1_preserve_header_case(&mut self, enabled: bool) -> &mut Self {
        self.inner.http1_preserve_header_case(enabled);
        self
    }

    /// Set a timeout for reading client request headers. If a client does not
    /// transmit the entire header within this time, the connection is closed.
    ///
    /// Default is None.
    pub fn http1_header_read_timeout(&mut self, val: Duration) -> &mut Self {
        self.inner.http1_header_read_timeout(val);
        self
    }

    /// Set whether HTTP/1 connections should try to use vectored writes,
    /// or always flatten into a single buffer.
    ///
    /// Note that setting this to false may mean more copies of body data,
    /// but may also improve performance when an IO transport doesn't
    /// support vectored writes well, such as most TLS implementations.
    ///
    /// Setting this to true will force hyper to use queued strategy
    /// which may eliminate unnecessary cloning on some TLS backends
    ///
    /// Default is `auto`. In this mode hyper will try to guess which
    /// mode to use
    pub fn http1_writev(&mut self, val: bool) -> &mut Self {
        self.inner.http1_writev(val);
        self
    }

    /// Sets whether HTTP2 is required.
    ///
    /// Default is false
    pub fn http2_only(&mut self, val: bool) -> &mut Self {
        self.inner.http2_only(val);
        self
    }

    /// Sets the [`SETTINGS_INITIAL_WINDOW_SIZE`][spec] option for HTTP2
    /// stream-level flow control.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    ///
    /// [spec]: https://http2.github.io/http2-spec/#SETTINGS_INITIAL_WINDOW_SIZE
    pub fn http2_initial_stream_window_size(&mut self, sz: impl Into<Option<u32>>) -> &mut Self {
        self.inner.http2_initial_stream_window_size(sz);
        self
    }

    /// Sets the max connection-level flow control for HTTP2.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    pub fn http2_initial_connection_window_size(
        &mut self,
        sz: impl Into<Option<u32>>,
    ) -> &mut Self {
        self.inner.http2_initial_connection_window_size(sz);
        self
    }

    /// Sets whether to use an adaptive flow control.
    ///
    /// Enabling this will override the limits set in
    /// `http2_initial_stream_window_size` and
    /// `http2_initial_connection_window_size`.
    pub fn http2_adaptive_window(&mut self, enabled: bool) -> &mut Self {
        self.inner.http2_adaptive_window(enabled);
        self
    }

    /// Sets the maximum frame size to use for HTTP2.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    pub fn http2_max_frame_size(&mut self, sz: impl Into<Option<u32>>) -> &mut Self {
        self.inner.http2_max_frame_size(sz);
        self
    }

    /// Sets the [`SETTINGS_MAX_CONCURRENT_STREAMS`][spec] option for HTTP2
    /// connections.
    ///
    /// Default is no limit (`std::u32::MAX`). Passing `None` will do nothing.
    ///
    /// [spec]: https://http2.github.io/http2-spec/#SETTINGS_MAX_CONCURRENT_STREAMS
    pub fn http2_max_concurrent_streams(&mut self, max: impl Into<Option<u32>>) -> &mut Self {
        self.inner.http2_max_concurrent_streams(max);
        self
    }

    /// Sets an interval for HTTP2 Ping frames should be sent to keep a
    /// connection alive.
    ///
    /// Pass `None` to disable HTTP2 keep-alive.
    ///
    /// Default is currently disabled.
    pub fn http2_keep_alive_interval(
        &mut self,
        interval: impl Into<Option<Duration>>,
    ) -> &mut Self {
        self.inner.http2_keep_alive_interval(interval);
        self
    }

    /// Sets a timeout for receiving an acknowledgement of the keep-alive ping.
    ///
    /// If the ping is not acknowledged within the timeout, the connection will
    /// be closed. Does nothing if `http2_keep_alive_interval` is disabled.
    ///
    /// Default is 20 seconds.
    pub fn http2_keep_alive_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.inner.http2_keep_alive_timeout(timeout);
        self
    }

    /// Set the maximum buffer size for the connection.
    ///
    /// Default is ~400kb.
    ///
    /// # Panics
    ///
    /// The minimum value allowed is 8192. This method panics if the passed `max` is less than the minimum.
    pub fn max_buf_size(&mut self, max: usize) -> &mut Self {
        self.inner.max_buf_size(max);
        self
    }

    /// Aggregates flushes to better support pipelined responses.
    ///
    /// Experimental, may have bugs.
    ///
    /// Default is false.
    pub fn pipeline_flush(&mut self, enabled: bool) -> &mut Self {
        self.inner.pipeline_flush(enabled);
        self
    }
}
