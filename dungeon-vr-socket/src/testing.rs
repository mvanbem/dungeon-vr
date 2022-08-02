use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::future::{pending, Future};
use std::hash::Hash;
use std::io;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::mpsc;

use crate::BoundSocket;

#[derive(Clone)]
pub struct FakeNetwork<A> {
    inner: Arc<Mutex<InnerFakeNetwork<A>>>,
}

struct InnerFakeNetwork<A> {
    bindings: HashMap<A, mpsc::UnboundedSender<(Vec<u8>, A)>>,
}

impl<A> FakeNetwork<A>
where
    A: Copy + Eq + Hash,
{
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerFakeNetwork {
                bindings: HashMap::new(),
            })),
        }
    }

    pub fn bind(&self, addr: A) -> FakeSocket<A> {
        let mut inner = self.inner.lock().unwrap();
        assert!(!inner.bindings.contains_key(&addr));

        let (tx, rx) = mpsc::unbounded_channel();
        inner.bindings.insert(addr, tx);

        FakeSocket {
            network: Arc::downgrade(&self.inner),
            local_addr: addr,
            rx: tokio::sync::Mutex::new(rx),
        }
    }
}

pub struct FakeSocket<A> {
    network: Weak<Mutex<InnerFakeNetwork<A>>>,
    local_addr: A,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<(Vec<u8>, A)>>,
}

impl<A> BoundSocket for FakeSocket<A>
where
    A: Debug + Display + Copy + Eq + Hash + Send + Sync + 'static,
{
    type Addr = A;
    type RecvFrom<'a> = impl Future<Output = io::Result<(usize, A)>> + 'a;
    type SendTo<'a> = impl Future<Output = io::Result<()>> + 'a;

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> Self::RecvFrom<'a> {
        async move {
            let mut rx = self.rx.lock().await;
            match rx.recv().await {
                Some((data, addr)) => {
                    buf[..data.len()].copy_from_slice(&data);
                    Ok((data.len(), addr))
                }
                None => pending().await,
            }
        }
    }

    fn send_to<'a>(&'a self, buf: &'a [u8], addr: A) -> Self::SendTo<'a> {
        async move {
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
}
