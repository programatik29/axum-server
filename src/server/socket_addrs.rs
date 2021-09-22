use std::{
    fmt, io,
    net::{SocketAddr, ToSocketAddrs},
};

type BoxedIterator = Box<dyn Iterator<Item = SocketAddr>>;

pub(crate) struct Boxed(Box<dyn ToSocketAddrs<Iter = BoxedIterator> + Send>);

impl Boxed {
    pub(crate) fn new<A, I>(addr: A) -> Self
    where
        A: ToSocketAddrs<Iter = I> + Send + 'static,
        I: Iterator<Item = SocketAddr> + 'static,
    {
        let boxed_iter = addr.map(|iter| Box::new(iter) as BoxedIterator);

        Self(Box::new(boxed_iter))
    }
}

impl fmt::Debug for Boxed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToSocketAddrs").finish()
    }
}

impl ToSocketAddrs for Boxed {
    type Iter = BoxedIterator;

    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        self.0.to_socket_addrs()
    }
}

struct Map<T, F> {
    inner: T,
    f: F,
}

impl<T, F, I> ToSocketAddrs for Map<T, F>
where
    T: ToSocketAddrs,
    F: Fn(T::Iter) -> I,
    I: Iterator<Item = SocketAddr>,
{
    type Iter = I;

    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        match self.inner.to_socket_addrs() {
            Ok(iter) => Ok((self.f)(iter)),
            Err(e) => Err(e),
        }
    }
}

trait ToSocketAddrsExt: ToSocketAddrs {
    fn map<F>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
    {
        Map { inner: self, f }
    }
}

impl<T> ToSocketAddrsExt for T where T: ToSocketAddrs {}

#[cfg(test)]
mod tests {
    use crate::Server;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

    #[test]
    fn check_all_types() {
        let addrs: &'static [SocketAddr] = Box::leak(Box::new([
            SocketAddr::from(([127, 0, 0, 1], 3000)),
            SocketAddr::from(([127, 0, 0, 1], 3001)),
        ]));

        Server::new()
            .bind(SocketAddr::from(([127, 0, 0, 1], 3000)))
            .bind("127.0.0.1:3000")
            .bind(("127.0.0.1", 3000))
            .bind((IpAddr::from([127, 0, 0, 1]), 3000))
            .bind(("127.0.0.1".to_owned(), 3000))
            .bind((Ipv4Addr::new(127, 0, 0, 1), 3000))
            .bind((Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), 3000))
            .bind("127.0.0.1:3000".to_owned())
            .bind(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 3000))
            .bind(SocketAddrV6::new(
                Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1),
                3000,
                0,
                0,
            ))
            .bind(addrs);
    }
}
