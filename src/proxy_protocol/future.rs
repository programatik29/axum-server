//! Future types.
use crate::accept::Accept;
use crate::proxy_protocol::{ForwardClientIp, Peekable};
use pin_project_lite::pin_project;
use std::{
    fmt,
    future::Future,
    io,
    net::IpAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};

pin_project! {
    /// Future type for [`ProxyProtocolAcceptor`](crate::proxy_protocol::ProxyProtocolAcceptor).
    pub struct ProxyProtocolAcceptorFuture<F, A, I, S>
    where
        A: Accept<I, S>,
        I: Peekable,
    {
        #[pin]
        inner: AcceptFuture<F, A, I, S>,
    }
}

impl<F, A, I, S> ProxyProtocolAcceptorFuture<F, A, I, S>
where
    A: Accept<I, S>,
    I: AsyncRead + AsyncWrite + Unpin + Peekable + 'static,
{
    pub(crate) fn new(read_header_future: F, acceptor: A, stream: I, service: S) -> Self {
        let inner = AcceptFuture::ReadHeader {
            read_header_future,
            acceptor,
            stream,
            service,
        };
        Self { inner }
    }
}

impl<F, A, I, S> fmt::Debug for ProxyProtocolAcceptorFuture<F, A, I, S>
where
    A: Accept<I, S>,
    I: AsyncRead + AsyncWrite + Unpin + Peekable,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyProtocolAcceptorFuture").finish()
    }
}

pin_project! {
    #[project = AcceptFutureProj]
    enum AcceptFuture<F, A, I, S>
    where
        A: Accept<I, S>,
        I: Peekable,
    {
        ReadHeader {
            #[pin]
            read_header_future: F,
            acceptor: A,
            stream: I,
            service: S,
        },
        ForwardIp {
            #[pin]
            inner_accept_future: A::Future,
            client_address_opt: Option<IpAddr>,
        },
    }
}

impl<F, A, I, S> Future for ProxyProtocolAcceptorFuture<F, A, I, S>
where
    A: Accept<I, S>,
    I: AsyncRead + AsyncWrite + Unpin + Peekable + 'static,
    F: Future<Output = Result<IpAddr, io::Error>>,
{
    type Output = io::Result<(A::Stream, ForwardClientIp<A::Service>)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.inner.as_mut().project() {
                AcceptFutureProj::ReadHeader {
                    read_header_future,
                    acceptor,
                    stream,
                    service
                } => match read_header_future.poll(cx) {
                    Poll::Ready(Ok(client_address)) => {
                        let client_address_opt = Some(client_address);

                        let inner_accept_future = acceptor.accept(*stream, *service);

                        this.inner.set(AcceptFuture::ForwardIp {
                            inner_accept_future,
                            client_address_opt,
                        });
                    }
                    Poll::Ready(Err(_)) => {
                        let client_address_opt = None;

                        let inner_accept_future = acceptor.accept(*stream, *service);

                        this.inner.set(AcceptFuture::ForwardIp {
                            inner_accept_future,
                            client_address_opt,
                        });
                    }
                    Poll::Pending => return Poll::Pending,
                },
                AcceptFutureProj::ForwardIp {
                    inner_accept_future,
                    client_address_opt,
                } => match inner_accept_future.poll(cx) {
                    Poll::Ready(Ok((stream, service))) => {
                        let service = ForwardClientIp {
                            inner: service,
                            client_address_opt: *client_address_opt,
                        };

                        return Poll::Ready(Ok((stream, service)));
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}
