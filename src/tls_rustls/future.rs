//! Future types.

use crate::tls_rustls::RustlsConfig;
use http::uri::Scheme;
use pin_project_lite::pin_project;
use std::{
    fmt,
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::{server::TlsStream, Accept, TlsAcceptor};
use tower_http::add_extension::AddExtension;

pin_project! {
    /// Future type for [`RustlsAcceptor`](crate::tls_rustls::RustlsAcceptor).
    pub struct RustlsAcceptorFuture<F, I, S> {
        #[pin]
        inner: AcceptFuture<F, I, S>,
        config: Option<RustlsConfig>,
    }
}

impl<F, I, S> RustlsAcceptorFuture<F, I, S> {
    pub(crate) fn new(future: F, config: RustlsConfig) -> Self {
        let inner = AcceptFuture::Inner { future };
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
        },
        Accept {
            #[pin]
            future: Accept<I>,
            service: Option<S>,
        },
    }
}

impl<F, I, S> Future for RustlsAcceptorFuture<F, I, S>
where
    F: Future<Output = io::Result<(I, S)>>,
    I: AsyncRead + AsyncWrite + Unpin,
{
    type Output = io::Result<(TlsStream<I>, AddExtension<S, Scheme>)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.inner.as_mut().project() {
                AcceptFutureProj::Inner { future } => {
                    match future.poll(cx) {
                        Poll::Ready(Ok((stream, service))) => {
                            let server_config = this.config
                                .take()
                                .expect("config is not set. this is a bug in axum-server, please report")
                                .get_inner();

                            let acceptor = TlsAcceptor::from(server_config);
                            let future = acceptor.accept(stream);

                            let service = Some(service);

                            this.inner.set(AcceptFuture::Accept { future, service });
                        }
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                AcceptFutureProj::Accept { future, service } => match future.poll(cx) {
                    Poll::Ready(Ok(stream)) => {
                        let service = service.take().expect("future polled after ready");
                        let service = AddExtension::new(service, Scheme::HTTPS);

                        return Poll::Ready(Ok((stream, service)));
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}
