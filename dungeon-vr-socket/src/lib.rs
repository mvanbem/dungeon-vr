#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

use std::fmt::{Debug, Display};
use std::future::Future;
use std::hash::Hash;
use std::io;
use std::net::SocketAddr;

use futures::TryFutureExt;
use tokio::net::UdpSocket;

pub mod testing;

pub trait BoundSocket {
    type Addr: Debug + Display + Copy + Eq + Hash + Send + Sync + 'static;

    type RecvFrom<'a>: Future<Output = io::Result<(usize, Self::Addr)>> + Send
    where
        Self: 'a;

    type SendTo<'a>: Future<Output = io::Result<()>> + Send
    where
        Self: 'a;

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> Self::RecvFrom<'a>;
    fn send_to<'a>(&'a self, buf: &'a [u8], addr: Self::Addr) -> Self::SendTo<'a>;
}

impl BoundSocket for UdpSocket {
    type Addr = SocketAddr;
    type RecvFrom<'a> = impl Future<Output = io::Result<(usize, SocketAddr)>> + 'a;
    type SendTo<'a> = impl Future<Output = io::Result<()>> + 'a;

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> Self::RecvFrom<'a> {
        self.recv_from(buf)
    }

    fn send_to<'a>(&'a self, buf: &'a [u8], addr: SocketAddr) -> Self::SendTo<'a> {
        self.send_to(buf, addr).map_ok(|_| ())
    }
}

pub trait ConnectedSocket {
    type Recv<'a>: Future<Output = io::Result<usize>> + Send
    where
        Self: 'a;

    type Send<'a>: Future<Output = io::Result<()>> + Send
    where
        Self: 'a;

    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> Self::Recv<'a>;
    fn send<'a>(&'a self, buf: &'a [u8]) -> Self::Send<'a>;
}

impl ConnectedSocket for UdpSocket {
    type Recv<'a> = impl Future<Output = io::Result<usize>> + 'a;
    type Send<'a> = impl Future<Output = io::Result<()>> + 'a;

    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> Self::Recv<'a> {
        self.recv(buf)
    }

    fn send<'a>(&'a self, buf: &'a [u8]) -> Self::Send<'a> {
        self.send(buf).map_ok(|_| ())
    }
}
