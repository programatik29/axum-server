//! Future types.

use super::OpenSSLConfig;
use pin_project_lite::pin_project;
use std::io::{Error, ErrorKind};
use std::time::Duration;
use std::{
    fmt,
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{timeout, Timeout};

use openssl::ssl::Ssl;
use tokio_openssl::SslStream;

pin_project! {
    /// Future type for [`OpenSSLAcceptor`](crate::tls_openssl::OpenSSLAcceptor).
    pub struct OpenSSLAcceptorFuture<F, I, S> {
        #[pin]
        inner: AcceptFuture<F, I, S>,
        config: Option<OpenSSLConfig>,
    }
}

impl<F, I, S> OpenSSLAcceptorFuture<F, I, S> {
    pub(crate) fn new(future: F, config: OpenSSLConfig, handshake_timeout: Duration) -> Self {
        let inner = AcceptFuture::InnerAccepting {
            future,
            handshake_timeout,
        };
        let config = Some(config);

        Self { inner, config }
    }
}

impl<F, I, S> fmt::Debug for OpenSSLAcceptorFuture<F, I, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSSLAcceptorFuture").finish()
    }
}

pin_project! {
    struct TlsAccept<I> {
        #[pin]
        tls_stream: Option<SslStream<I>>,
    }
}

impl<I> Future for TlsAccept<I>
where
    I: AsyncRead + AsyncWrite + Unpin,
{
    type Output = io::Result<SslStream<I>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        match this
            .tls_stream
            .as_mut()
            .as_pin_mut()
            .map(|inner| inner.poll_accept(cx))
            .expect("tlsaccept polled after ready")
        {
            Poll::Ready(Ok(())) => {
                let tls_stream = this.tls_stream.take().expect("tls stream vanished?");

                Poll::Ready(Ok(tls_stream))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(Error::new(ErrorKind::Other, e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

pin_project! {
    #[project = AcceptFutureProj]
    enum AcceptFuture<F, I, S> {
        // We are waiting on the inner (lower) future to complete accept()
        // so that we can begin installing TLS into the channel.
        InnerAccepting {
            #[pin]
            future: F,
            handshake_timeout: Duration,
        },
        // We are waiting for TLS to install into the channel so that we can
        // proceed to return the SslStream.
        TlsAccepting {
            #[pin]
            future: Timeout< TlsAccept<I> >,
            service: Option<S>,
        }
    }
}

impl<F, I, S> Future for OpenSSLAcceptorFuture<F, I, S>
where
    F: Future<Output = io::Result<(I, S)>>,
    I: AsyncRead + AsyncWrite + Unpin,
{
    type Output = io::Result<(SslStream<I>, S)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // The inner future here is what is doing the lower level accept, such as
        // our tcp socket.
        //
        // So we poll on that first, when it's ready we then swap our the inner future to
        // one waiting for our ssl layer to accept/install.
        //
        // Then once that's ready we can then wrap and provide the SslStream back out.

        // This loop exists to allow the Poll::Ready from InnerAccept on complete
        // to re-poll immediately. Otherwise all other paths are immediate returns.
        loop {
            match this.inner.as_mut().project() {
                AcceptFutureProj::InnerAccepting {
                    future,
                    handshake_timeout,
                } => match future.poll(cx) {
                    Poll::Ready(Ok((stream, service))) => {
                        let server_config = this.config.take().expect(
                            "config is not set. this is a bug in axum-server, please report",
                        );

                        // Change to poll::ready(err)
                        let ssl = Ssl::new(server_config.get_inner().context()).unwrap();

                        let tls_stream = SslStream::new(ssl, stream).unwrap();
                        let future = TlsAccept {
                            tls_stream: Some(tls_stream),
                        };

                        let service = Some(service);
                        let handshake_timeout = *handshake_timeout;

                        this.inner.set(AcceptFuture::TlsAccepting {
                            future: timeout(handshake_timeout, future),
                            service,
                        });
                        // the loop is now triggered to immediately poll on
                        // ssl stream accept.
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                },

                AcceptFutureProj::TlsAccepting { future, service } => match future.poll(cx) {
                    Poll::Ready(Ok(Ok(stream))) => {
                        let service = service.take().expect("future polled after ready");

                        return Poll::Ready(Ok((stream, service)));
                    }
                    Poll::Ready(Ok(Err(e))) => return Poll::Ready(Err(e)),
                    Poll::Ready(Err(timeout)) => {
                        return Poll::Ready(Err(Error::new(ErrorKind::TimedOut, timeout)))
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}
