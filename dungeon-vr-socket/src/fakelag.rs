use std::io;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rand::{thread_rng, Rng};
use rand_distr::Exp;
use tokio::select;
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;

use crate::{AddrBound, BoundSocket, ConnectedSocket};

pub struct FakeLagBoundSocket<T, Addr> {
    _cancel_guard: cancel::Guard,
    inner: Arc<T>,
    packet_rx: Mutex<mpsc::UnboundedReceiver<(Vec<u8>, Addr)>>,
    delay_sec: Exp<f64>,
}

impl<T, Addr> FakeLagBoundSocket<T, Addr>
where
    T: BoundSocket<Addr>,
    Addr: AddrBound,
{
    pub fn new(inner: T, mean_delay: Duration) -> Self {
        let cancel_token = cancel::Token::new();
        let inner = Arc::new(inner);
        let (packet_tx, packet_rx) = mpsc::unbounded_channel();
        let delay_sec = Exp::new(1.0 / mean_delay.as_secs_f64()).unwrap();

        tokio::spawn(Self::run(
            cancel_token.clone(),
            Arc::clone(&inner),
            delay_sec,
            packet_tx,
        ));

        Self {
            _cancel_guard: cancel_token.guard(),
            inner,
            packet_rx: Mutex::new(packet_rx),
            delay_sec,
        }
    }

    async fn run(
        cancel_token: cancel::Token,
        inner: Arc<T>,
        delay_sec: Exp<f64>,
        packet_tx: tokio::sync::mpsc::UnboundedSender<(Vec<u8>, Addr)>,
    ) {
        let mut buf = [0; 65536];
        while !cancel_token.is_cancelled() {
            let result = select! {
                biased;
                _ = cancel_token.cancelled() => return,
                result = inner.recv_from(&mut buf[..]) => result,
            };
            match result {
                Ok((size, addr)) => {
                    tokio::spawn(Self::delay_recv(
                        Duration::from_secs_f64(thread_rng().sample(delay_sec)),
                        packet_tx.clone(),
                        buf[..size].to_vec(),
                        addr,
                    ));
                }
                Err(e) => {
                    log::warn!("Unexpected socket error: {e}");
                }
            }
        }
    }

    async fn delay_send(delay: Duration, inner: Arc<T>, data: Vec<u8>, addr: Addr) {
        sleep(delay).await;
        match inner.send_to(&data, addr).await {
            Ok(()) => (),
            Err(e) => {
                log::warn!("Unexpected socket error: {e}");
            }
        }
    }

    async fn delay_recv(
        delay: Duration,
        packet_tx: tokio::sync::mpsc::UnboundedSender<(Vec<u8>, Addr)>,
        data: Vec<u8>,
        addr: Addr,
    ) {
        sleep(delay).await;
        let _ = packet_tx.send((data, addr));
    }
}

#[async_trait]
impl<T, Addr> BoundSocket<Addr> for FakeLagBoundSocket<T, Addr>
where
    T: BoundSocket<Addr>,
    Addr: AddrBound,
{
    async fn recv_from(&'_ self, buf: &'_ mut [u8]) -> io::Result<(usize, Addr)> {
        let (data, addr) = self.packet_rx.lock().await.recv().await.unwrap();
        buf[..data.len()].copy_from_slice(&data);
        Ok((data.len(), addr))
    }

    async fn send_to(&'_ self, buf: &'_ [u8], addr: Addr) -> io::Result<()> {
        tokio::spawn(Self::delay_send(
            Duration::from_secs_f64(thread_rng().sample(self.delay_sec)),
            Arc::clone(&self.inner),
            buf.to_vec(),
            addr,
        ));
        Ok(())
    }
}

pub struct FakeLagConnectedSocket<T> {
    _cancel_guard: cancel::Guard,
    inner: Arc<T>,
    packet_rx: Mutex<mpsc::UnboundedReceiver<Vec<u8>>>,
    delay_sec: Exp<f64>,
}

impl<T> FakeLagConnectedSocket<T>
where
    T: ConnectedSocket,
{
    pub fn new(inner: T, mean_delay: Duration) -> Self {
        let cancel_token = cancel::Token::new();
        let inner = Arc::new(inner);
        let (packet_tx, packet_rx) = mpsc::unbounded_channel();
        let delay_sec = Exp::new(1.0 / mean_delay.as_secs_f64()).unwrap();

        tokio::spawn(Self::run(
            cancel_token.clone(),
            Arc::clone(&inner),
            delay_sec,
            packet_tx,
        ));

        Self {
            _cancel_guard: cancel_token.guard(),
            inner,
            packet_rx: Mutex::new(packet_rx),
            delay_sec,
        }
    }

    async fn run(
        cancel_token: cancel::Token,
        inner: Arc<T>,
        delay_sec: Exp<f64>,
        packet_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    ) {
        let mut buf = [0; 65536];
        while !cancel_token.is_cancelled() {
            let result = select! {
                biased;
                _ = cancel_token.cancelled() => return,
                result = inner.recv(&mut buf[..]) => result,
            };
            match result {
                Ok(size) => {
                    tokio::spawn(Self::delay_recv(
                        Duration::from_secs_f64(thread_rng().sample(delay_sec)),
                        packet_tx.clone(),
                        buf[..size].to_vec(),
                    ));
                }
                Err(e) => {
                    log::warn!("Unexpected socket error: {e}");
                }
            }
        }
    }

    async fn delay_send(delay: Duration, inner: Arc<T>, data: Vec<u8>) {
        sleep(delay).await;
        match inner.send(&data).await {
            Ok(()) => (),
            Err(e) => {
                log::warn!("Unexpected socket error: {e}");
            }
        }
    }

    async fn delay_recv(
        delay: Duration,
        packet_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        data: Vec<u8>,
    ) {
        sleep(delay).await;
        let _ = packet_tx.send(data);
    }
}

#[async_trait]
impl<T> ConnectedSocket for FakeLagConnectedSocket<T>
where
    T: ConnectedSocket,
{
    async fn recv(&'_ self, buf: &'_ mut [u8]) -> io::Result<usize> {
        let data = self.packet_rx.lock().await.recv().await.unwrap();
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    async fn send(&'_ self, buf: &'_ [u8]) -> io::Result<()> {
        tokio::spawn(Self::delay_send(
            Duration::from_secs_f64(thread_rng().sample(self.delay_sec)),
            Arc::clone(&self.inner),
            buf.to_vec(),
        ));
        Ok(())
    }
}
