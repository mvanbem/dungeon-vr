#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::io;

use async_trait::async_trait;

pub mod fakelag;
mod std_impls;
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
pub trait ConnectedSocket: Send + Sync + 'static {
    async fn recv(&'_ self, buf: &'_ mut [u8]) -> io::Result<usize>;
    async fn send(&'_ self, buf: &'_ [u8]) -> io::Result<()>;
}
