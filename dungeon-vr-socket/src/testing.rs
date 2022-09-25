use std::collections::HashMap;
use std::future::pending;
use std::io;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::{AddrBound, BoundSocket, ConnectedSocket};

#[derive(Clone)]
pub struct FakeNetwork<Addr> {
    inner: Arc<Mutex<InnerFakeNetwork<Addr>>>,
}

struct InnerFakeNetwork<Addr> {
    bindings: HashMap<Addr, mpsc::UnboundedSender<(Vec<u8>, Addr)>>,
}

impl<Addr> FakeNetwork<Addr>
where
    Addr: AddrBound,
{
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerFakeNetwork {
                bindings: HashMap::new(),
            })),
        }
    }

    pub fn bind(&self, addr: Addr) -> FakeBoundSocket<Addr> {
        let mut inner = self.inner.lock().unwrap();
        assert!(!inner.bindings.contains_key(&addr));

        let (tx, rx) = mpsc::unbounded_channel();
        inner.bindings.insert(addr, tx);

        FakeBoundSocket {
            network: Arc::downgrade(&self.inner),
            local_addr: addr,
            rx: tokio::sync::Mutex::new(rx),
        }
    }

    pub fn connect(&self, local_addr: Addr, remote_addr: Addr) -> FakeConnectedSocket<Addr> {
        let mut inner = self.inner.lock().unwrap();
        assert!(!inner.bindings.contains_key(&local_addr));

        let (tx, rx) = mpsc::unbounded_channel();
        inner.bindings.insert(local_addr, tx);

        FakeConnectedSocket {
            network: Arc::downgrade(&self.inner),
            local_addr,
            remote_addr,
            rx: tokio::sync::Mutex::new(rx),
        }
    }
}

pub struct FakeBoundSocket<Addr> {
    network: Weak<Mutex<InnerFakeNetwork<Addr>>>,
    local_addr: Addr,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<(Vec<u8>, Addr)>>,
}

#[async_trait]
impl<Addr> BoundSocket<Addr> for FakeBoundSocket<Addr>
where
    Addr: AddrBound,
{
    async fn recv_from(&'_ self, buf: &'_ mut [u8]) -> io::Result<(usize, Addr)> {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some((data, addr)) => {
                buf[..data.len()].copy_from_slice(&data);
                Ok((data.len(), addr))
            }
            None => pending().await,
        }
    }

    async fn send_to(&'_ self, buf: &'_ [u8], addr: Addr) -> io::Result<()> {
        let network = match self.network.upgrade() {
            Some(network) => network,
            None => return Ok(()),
        };
        let tx = match network.lock().unwrap().bindings.get(&addr) {
            Some(tx) => tx.clone(),
            None => return Ok(()),
        };
        drop(network);
        drop(tx.send((buf.to_vec(), self.local_addr)));
        Ok(())
    }
}

pub struct FakeConnectedSocket<Addr> {
    network: Weak<Mutex<InnerFakeNetwork<Addr>>>,
    local_addr: Addr,
    remote_addr: Addr,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<(Vec<u8>, Addr)>>,
}

#[async_trait]
impl<Addr> ConnectedSocket for FakeConnectedSocket<Addr>
where
    Addr: AddrBound,
{
    async fn recv(&'_ self, buf: &'_ mut [u8]) -> io::Result<usize> {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some((data, _addr)) => {
                buf[..data.len()].copy_from_slice(&data);
                Ok(data.len())
            }
            None => pending().await,
        }
    }

    async fn send(&'_ self, buf: &'_ [u8]) -> io::Result<()> {
        let network = match self.network.upgrade() {
            Some(network) => network,
            None => return Ok(()),
        };
        let tx = match network.lock().unwrap().bindings.get(&self.remote_addr) {
            Some(tx) => tx.clone(),
            None => return Ok(()),
        };
        drop(network);
        drop(tx.send((buf.to_vec(), self.local_addr)));
        Ok(())
    }
}
