//! Future types.
use crate::accept::Accept;
use crate::proxy_protocol::{read_proxy_header, ForwardClientIp, Peekable};
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
    pub struct ProxyProtocolAcceptorFuture<A: Accept<I, S>, I, S> {
        #[pin]
        inner: AcceptFuture<A, I, S>,
    }
}

impl<A, I, S> ProxyProtocolAcceptorFuture<A, I, S>
where
    A: Accept<I, S>,
    A::Stream: AsyncRead + AsyncWrite + Unpin + Peekable,
{
    pub(crate) fn new(inner_acceptor: A, inner_stream: I, inner_service: S) -> Self {
        // use timeout to wrap whole job

        let inner = AcceptFuture::ReadHeader {
            acceptor: inner_acceptor,
            stream: inner_stream,
            service: inner_service,
        };

        Self { inner }
    }
}

impl<A, I, S> fmt::Debug for ProxyProtocolAcceptorFuture<A, I, S>
where
    A: Accept<I, S>,
    A::Stream: AsyncRead + AsyncWrite + Unpin + Peekable,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyProtocolAcceptorFuture").fIsh()
    }
}

pin_project! {
    #[project = AcceptFutureProj]
    enum AcceptFuture<A: Accept<I, S>, I, S> {
        ReadHeader {
            acceptor: A,
            stream: I,
            service: S,
        },
        Inner {
            #[pin]
            future: A::Future,
            client_address_opt: Option<IpAddr>,
        }
    }
}

impl<A, I, S> Future for ProxyProtocolAcceptorFuture<A, I, S>
where
    A: Accept<I, S>,
    A::Stream: AsyncRead + AsyncWrite + Unpin + Peekable,
{
    type Output = io::Result<(I, ForwardClientIp<S>)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.inner.as_mut().project() {
                AcceptFutureProj::ReadHeader {
                    acceptor,
                    stream,
                    service,
                } => {
                    let read_proxy_future = read_proxy_header(&mut stream);

                    let client_address_opt = match read_proxy_future.poll(cx) {
                        Poll::Ready(Ok(client_address)) => Some(client_address),
                        Poll::Ready(Err(_)) => None,
                        Poll::Pending => return Poll::Pending,
                    };

                    let future = acceptor.accept(stream, service);

                    this.inner.set(AcceptFuture::Inner {
                        future,
                        client_address_opt,
                    });
                }
                AcceptFutureProj::Inner {
                    future,
                    client_address_opt,
                } => {
                    match future.poll(cx) {
                        Poll::Ready(Ok((stream, service))) => {
                            let service = ForwardClientIp {
                                inner: service,
                                client_address_opt: client_address_opt,
                            };

                            return Poll::Ready(Ok((stream, service)));
                        }
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
            }
        }
    }
}
