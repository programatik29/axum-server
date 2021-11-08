//! Service traits.

use http::Response;
use http_body::Body;
use std::{
    future::Future,
    task::{Context, Poll},
};
use tower_service::Service;

/// Modified version of [`MakeService`] that has some trait bounds and takes a `&Target`.
///
/// This trait is sealed and cannot be implemented for types outside this crate.
///
/// [`MakeService`]: https://docs.rs/tower/0.4/tower/make/trait.MakeService.html
#[allow(missing_docs)]
pub trait MakeServiceRef<Target, Request>: private::Sealed<(Target, Request)> {
    type Service: Service<
            Request,
            Response = Response<Self::Body>,
            Error = Self::Error,
            Future = Self::Future,
        > + Send
        + 'static;

    type Body: Body<Data = Self::Data, Error = Self::BodyError> + Send + 'static;
    type Data: Send + 'static;
    type BodyError: Into<Box<dyn std::error::Error + Send + Sync>>;

    type Error: Into<Box<dyn std::error::Error + Send + Sync>>;

    type Future: Future<Output = Result<Response<Self::Body>, Self::Error>> + Send + 'static;

    type MakeError: Into<Box<dyn std::error::Error + Send + Sync>>;
    type MakeFuture: Future<Output = Result<Self::Service, Self::MakeError>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::MakeError>>;

    fn make_service(&mut self, target: &Target) -> Self::MakeFuture;
}

impl<T, S, B, E, F, Target, Request> private::Sealed<(Target, Request)> for T
where
    T: for<'a> Service<&'a Target, Response = S, Error = E, Future = F>,
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

impl<T, S, B, E, F, Target, Request> MakeServiceRef<Target, Request> for T
where
    T: for<'a> Service<&'a Target, Response = S, Error = E, Future = F>,
    S: Service<Request, Response = Response<B>> + Send + 'static,
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
    type Data = B::Data;
    type BodyError = B::Error;

    type Error = S::Error;

    type Future = S::Future;

    type MakeError = E;
    type MakeFuture = F;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::MakeError>> {
        self.poll_ready(cx)
    }

    fn make_service(&mut self, target: &Target) -> Self::MakeFuture {
        self.call(target)
    }
}

mod private {
    pub trait Sealed<T> {}
}
