use std::io;
use std::net::SocketAddr;

use async_trait::async_trait;
use futures::TryFutureExt;
use tokio::net::UdpSocket;

use crate::{BoundSocket, ConnectedSocket};

#[async_trait]
impl BoundSocket<SocketAddr> for UdpSocket {
    async fn recv_from(&'_ self, buf: &'_ mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.recv_from(buf).await
    }

    async fn send_to(&'_ self, buf: &'_ [u8], addr: SocketAddr) -> io::Result<()> {
        self.send_to(buf, addr).map_ok(|_| ()).await
    }
}

#[async_trait]
impl ConnectedSocket for UdpSocket {
    async fn recv(&'_ self, buf: &'_ mut [u8]) -> io::Result<usize> {
        self.recv(buf).await
    }

    async fn send(&'_ self, buf: &'_ [u8]) -> io::Result<()> {
        self.send(buf).map_ok(|_| ()).await
    }
}
