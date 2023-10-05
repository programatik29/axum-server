//! Future types.
use crate::accept::Accept;
use crate::proxy_protocol::Peekable;
use crate::proxy_protocol::{read_proxy_header, ForwardClientIp};
use pin_project_lite::pin_project;
use std::io::{Error, ErrorKind};
use std::time::Duration;
use std::{
    fmt,
    future::Future,
    io,
    net::IpAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{timeout, Timeout};

pin_project! {
    /// Future type for [`ProxyProtocolAcceptor`](crate::proxy_protocol::ProxyProtocolAcceptor).
    pub struct ProxyProtocolAcceptorFuture<A, I, S>
    where
        A: Accept<I, S>
    {
        #[pin]
        inner: AcceptFuture<A, I, S>,
    }
}

impl<A, I, S> ProxyProtocolAcceptorFuture<A, I, S>
where
    A: Accept<I, S>,
{
    pub(crate) fn new(
        inner_acceptor: A,
        inner_stream: I,
        inner_service: S,
        _job_timeout: Duration,
    ) -> Self {
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
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyProtocolAcceptorFuture").finish()
    }
}

pin_project! {
    #[project = AcceptFutureProj]
    enum AcceptFuture<A, I, S>
    where
        A: Accept<I, S>
    {
        ReadHeader {
            acceptor: A,
            #[pin]
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
    I: AsyncRead + AsyncWrite + Unpin + Peekable,
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
                    let future = acceptor.accept(stream, service);

                    match read_proxy_future.poll(cx) {
                        Poll::Ready(Ok(client_address)) => {
                            this.inner.set(AcceptFuture::Inner {
                                future: future,
                                client_address_opt: Some(client_address),
                            });
                        }
                        Poll::Ready(Err(_e)) => {
                            this.inner.set(AcceptFuture::Inner {
                                future: future,
                                client_address_opt: None,
                            });
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                AcceptFutureProj::Inner {
                    future,
                    client_address_opt,
                } => {
                    let (stream, service) = match future.as_mut().poll(cx) {
                        Poll::Ready(Ok((stream, service))) => (stream, service),
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    };

                    // Return the result, wrapped in the ForwardClientIp middleware.
                    return Poll::Ready(Ok((
                        stream,
                        ForwardClientIp {
                            inner: service,
                            client_address_opt: client_address_opt.clone(),
                        },
                    )));
                }
            }
        }
    }
}
