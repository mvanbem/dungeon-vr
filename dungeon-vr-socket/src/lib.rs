#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::io;
use std::net::SocketAddr;

use async_trait::async_trait;
use futures::TryFutureExt;
use tokio::net::UdpSocket;

pub mod testing;

pub trait AddrBound: Debug + Display + Copy + Eq + Hash + Send + Sync + 'static {}

impl<T> AddrBound for T where T: Debug + Display + Copy + Eq + Hash + Send + Sync + 'static {}

#[async_trait]
pub trait BoundSocket<Addr>: Send + Sync + 'static {
    async fn recv_from(&'_ self, buf: &'_ mut [u8]) -> io::Result<(usize, Addr)>
    where
        Addr: AddrBound;
    async fn send_to(&'_ self, buf: &'_ [u8], addr: Addr) -> io::Result<()>
    where
        Addr: AddrBound;
}

#[async_trait]
impl BoundSocket<SocketAddr> for UdpSocket {
    async fn recv_from(&'_ self, buf: &'_ mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.recv_from(buf).await
    }

    async fn send_to(&'_ self, buf: &'_ [u8], addr: SocketAddr) -> io::Result<()> {
        self.send_to(buf, addr).map_ok(|_| ()).await
    }
}

// pub trait ConnectedSocket {
//     type Recv<'a>: Future<Output = io::Result<usize>> + Send
//     where
//         Self: 'a;

//     type Send<'a>: Future<Output = io::Result<()>> + Send
//     where
//         Self: 'a;

//     fn recv<'a>(&'a self, buf: &'a mut [u8]) -> Self::Recv<'a>;
//     fn send<'a>(&'a self, buf: &'a [u8]) -> Self::Send<'a>;
// }

// impl ConnectedSocket for UdpSocket {
//     type Recv<'a> = impl Future<Output = io::Result<usize>> + 'a;
//     type Send<'a> = impl Future<Output = io::Result<()>> + 'a;

//     fn recv<'a>(&'a self, buf: &'a mut [u8]) -> Self::Recv<'a> {
//         self.recv(buf)
//     }

//     fn send<'a>(&'a self, buf: &'a [u8]) -> Self::Send<'a> {
//         self.send(buf).map_ok(|_| ())
//     }
// }
