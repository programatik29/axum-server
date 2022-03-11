//! Future types.

use crate::tls_rustls::RustlsConfig;
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
use tokio_rustls::{server::TlsStream, Accept, TlsAcceptor};

pin_project! {
    /// Future type for [`RustlsAcceptor`](crate::tls_rustls::RustlsAcceptor).
    pub struct RustlsAcceptorFuture<F, I, S> {
        #[pin]
        inner: AcceptFuture<F, I, S>,
        config: Option<RustlsConfig>,
    }
}

impl<F, I, S> RustlsAcceptorFuture<F, I, S> {
    pub(crate) fn new(future: F, config: RustlsConfig, handshake_timeout: Duration) -> Self {
        let inner = AcceptFuture::Inner {
            future,
            handshake_timeout,
        };
        let config = Some(config);

        Self { inner, config }
    }
}

impl<F, I, S> fmt::Debug for RustlsAcceptorFuture<F, I, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustlsAcceptorFuture").finish()
    }
}

pin_project! {
    #[project = AcceptFutureProj]
    enum AcceptFuture<F, I, S> {
        Inner {
            #[pin]
            future: F,
            handshake_timeout: Duration,
        },
        Accept {
            #[pin]
            future: Timeout<Accept<I>>,
            service: Option<S>,
        },
    }
}

impl<F, I, S> Future for RustlsAcceptorFuture<F, I, S>
where
    F: Future<Output = io::Result<(I, S)>>,
    I: AsyncRead + AsyncWrite + Unpin,
{
    type Output = io::Result<(TlsStream<I>, S)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.inner.as_mut().project() {
                AcceptFutureProj::Inner {
                    future,
                    handshake_timeout,
                } => {
                    match future.poll(cx) {
                        Poll::Ready(Ok((stream, service))) => {
                            let server_config = this.config
                                .take()
                                .expect("config is not set. this is a bug in axum-server, please report")
                                .get_inner();

                            let acceptor = TlsAcceptor::from(server_config);
                            let future = acceptor.accept(stream);

                            let service = Some(service);
                            let handshake_timeout = *handshake_timeout;

                            this.inner.set(AcceptFuture::Accept {
                                future: timeout(handshake_timeout, future),
                                service,
                            });
                        }
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                AcceptFutureProj::Accept { future, service } => match future.poll(cx) {
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
