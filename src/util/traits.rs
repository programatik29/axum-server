use http_body::Body;
use hyper::Response;
use std::future::Future;
use tower_service::Service;

pub(crate) trait HyperService<Request>
where
    Self: Service<
            Request,
            Response = Response<<Self as HyperService<Request>>::RespBody>,
            Future = <Self as HyperService<Request>>::SendFuture,
            Error = <Self as HyperService<Request>>::BoxedError,
        >
        + Send
        + Sync
        + 'static
        + Clone,
{
    type SendFuture: Future<Output = Result<Self::Response, Self::Error>> + Send + 'static;
    type BoxedError: Into<Box<dyn std::error::Error + Send + Sync>>;
    type RespBody: SendBody;
}

impl<T, B, Request> HyperService<Request> for T
where
    T: Service<Request, Response = Response<B>> + Send + Sync + 'static + Clone,
    T::Future: Future<Output = Result<Self::Response, Self::Error>> + Send + 'static,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    B: SendBody,
{
    type SendFuture = T::Future;
    type BoxedError = T::Error;
    type RespBody = B;
}

pub(crate) trait SendBody
where
    Self: Body<Data = <Self as SendBody>::SendData, Error = <Self as SendBody>::BoxedError>
        + Send
        + 'static,
{
    type SendData: Send;
    type BoxedError: Into<Box<dyn std::error::Error + Send + Sync>>;
}

impl<T> SendBody for T
where
    T: Body + Send + 'static,
    T::Data: Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type SendData = T::Data;
    type BoxedError = T::Error;
}
