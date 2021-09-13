use std::net::{SocketAddr, ToSocketAddrs};

pub(crate) type BoxedIterator = Box<dyn Iterator<Item = SocketAddr>>;

pub(crate) type Boxed = Box<dyn ToSocketAddrs<Iter = BoxedIterator> + Send>;

impl<T> ToSocketAddrsExt for T where T: ToSocketAddrs {}

pub(crate) trait ToSocketAddrsExt: ToSocketAddrs {
    fn map<F>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
    {
        Map { inner: self, f }
    }
}

pub(crate) struct Map<T, F> {
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

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        match self.inner.to_socket_addrs() {
            Ok(iter) => Ok((self.f)(iter)),
            Err(e) => Err(e),
        }
    }
}

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
