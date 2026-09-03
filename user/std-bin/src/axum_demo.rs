#[cfg(target_os = "scarlet")]
use std::io;
#[cfg(target_os = "scarlet")]
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

#[cfg(target_os = "scarlet")]
use axum::{Router, routing::get};
#[cfg(target_os = "scarlet")]
use tokio::net::TcpListener;

#[cfg(target_os = "scarlet")]
const LISTEN_PORT: u16 = 8080;

#[cfg(target_os = "scarlet")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let app = Router::new().route("/", get(root));
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, LISTEN_PORT));
    let listener = TcpListener::bind(address).await?;

    println!("Axum listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app).await
}

#[cfg(target_os = "scarlet")]
async fn root() -> &'static str {
    "Hello from Axum on Scarlet!\n"
}

#[cfg(not(target_os = "scarlet"))]
fn main() {
    eprintln!("axum-demo is only available on Scarlet");
}
