use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;

use anyhow::Result;
use clap::Parser;
use dungeon_vr_server_connection::ServerConnection;
use tokio::net::UdpSocket;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Server bind IPv4 address.
    #[clap(long)]
    ip: Option<String>,

    /// Server UDP port.
    #[clap(long, default_value = "7777")]
    port: u16,
}

#[tokio::main]
pub async fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_target(false)
        .init();
    let args = Args::parse();

    let ip = match &args.ip {
        Some(addr) => Ipv4Addr::from_str(addr)?,
        None => Ipv4Addr::UNSPECIFIED,
    };
    let socket = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(ip, args.port))).await?;
    log::info!("Listening on {}", socket.local_addr()?);
    let (_cancel_guard, _requests, mut events) = ServerConnection::spawn(socket);

    while let Some(_event) = events.recv().await {
        //
    }
    Ok(())
}
