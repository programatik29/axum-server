//! Service traits.

use http::Response;
use http_body::Body;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{util::Oneshot, ServiceExt};
use tower_service::Service;

/// Trait alias for [`Service`] with bounds required for [`serve`](crate::server::Server::serve).
///
/// This trait is sealed and cannot be implemented for types outside this crate.
#[allow(missing_docs)]
pub trait SendService<Request>: send_service::Sealed<Request> {
    type Service: Service<
            Request,
            Response = Response<Self::Body>,
            Error = Self::Error,
            Future = Self::Future,
        > + Send
        + Clone
        + 'static;

    type Body: Body<Data = Self::BodyData, Error = Self::BodyError> + Send + 'static;
    type BodyData: Send + 'static;
    type BodyError: Into<Box<dyn std::error::Error + Send + Sync>>;

    type Error: Into<Box<dyn std::error::Error + Send + Sync>>;

    type Future: Future<Output = Result<Response<Self::Body>, Self::Error>> + Send + 'static;

    fn into_service(self) -> Self::Service;
}

impl<T, B, Request> send_service::Sealed<Request> for T
where
    T: Service<Request, Response = Response<B>>,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::Future: Send + 'static,
    B: Body + Send + 'static,
    B::Data: Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
}

impl<T, B, Request> SendService<Request> for T
where
    T: Service<Request, Response = Response<B>> + Send + Clone + 'static,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::Future: Send + 'static,
    B: Body + Send + 'static,
    B::Data: Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Service = T;

    type Body = B;
    type BodyData = B::Data;
    type BodyError = B::Error;

    type Error = T::Error;

    type Future = T::Future;

    fn into_service(self) -> Self::Service {
        self
    }
}

/// Modified version of [`MakeService`] that takes a `&Target` and has required trait bounds for
/// [`serve`](crate::server::Server::serve).
///
/// This trait is sealed and cannot be implemented for types outside this crate.
///
/// [`MakeService`]: https://docs.rs/tower/0.4/tower/make/trait.MakeService.html
#[allow(missing_docs)]
pub trait MakeService<Target, Request>: make_service_ref::Sealed<(Target, Request)> {
    type Service: Service<
            Request,
            Response = Response<Self::Body>,
            Error = Self::Error,
            Future = Self::Future,
        >
        + Send
        + 'static
        + Clone;

    type Body: Body<Data = Self::BodyData, Error = Self::BodyError> + Send + 'static;
    type BodyData: Send + 'static;
    type BodyError: Into<Box<dyn std::error::Error + Send + Sync>>;

    type Error: Into<Box<dyn std::error::Error + Send + Sync>>;

    type Future: Future<Output = Result<Response<Self::Body>, Self::Error>> + Send + 'static;

    type MakeError: Into<Box<dyn std::error::Error + Send + Sync>>;
    type MakeFuture: Future<Output = Result<Self::Service, Self::MakeError>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::MakeError>>;

    fn make_service(&mut self, target: Target) -> Self::MakeFuture;
}

impl<T, S, B, E, F, Target, Request> make_service_ref::Sealed<(Target, Request)> for T
where
    T: Service<Target, Response = S, Error = E, Future = F>,
    S: Service<Request, Response = Response<B>> + Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: Send + 'static,
    B: Body + Send + 'static,
    B::Data: Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
    F: Future<Output = Result<S, E>>,
{
}

impl<T, S, B, E, F, Target, Request> MakeService<Target, Request> for T
where
    T: Service<Target, Response = S, Error = E, Future = F>,
    S: Service<Request, Response = Response<B>> + Send + Clone + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: Send + 'static,
    B: Body + Send + 'static,
    B::Data: Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
    F: Future<Output = Result<S, E>>,
{
    type Service = S;

    type Body = B;
    type BodyData = B::Data;
    type BodyError = B::Error;

    type Error = S::Error;

    type Future = S::Future;

    type MakeError = E;
    type MakeFuture = F;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::MakeError>> {
        self.poll_ready(cx)
    }

    fn make_service(&mut self, target: Target) -> Self::MakeFuture {
        self.call(target)
    }
}

mod send_service {
    pub trait Sealed<T> {}
}

mod make_service_ref {
    pub trait Sealed<T> {}
}

/// A tower service converted into a hyper service.
#[derive(Debug, Copy, Clone)]
pub(crate) struct TowerToHyperService<S> {
    service: S,
}

impl<S> TowerToHyperService<S> {
    /// Create a new `TowerToHyperService` from a tower service.
    pub(crate) fn new(tower_service: S) -> Self {
        Self {
            service: tower_service,
        }
    }
}

impl<S, R> hyper::service::Service<R> for TowerToHyperService<S>
where
    S: tower_service::Service<R> + Clone,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = TowerToHyperServiceFuture<S, R>;

    fn call(&self, req: R) -> Self::Future {
        TowerToHyperServiceFuture {
            future: self.service.clone().oneshot(req),
        }
    }
}

pin_project! {
    /// Response future for [`TowerToHyperService`].
    pub struct TowerToHyperServiceFuture<S, R>
    where
        S: tower_service::Service<R>,
    {
        #[pin]
        future: Oneshot<S, R>,
    }
}

impl<S, R> Future for TowerToHyperServiceFuture<S, R>
where
    S: tower_service::Service<R>,
{
    type Output = Result<S::Response, S::Error>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}
