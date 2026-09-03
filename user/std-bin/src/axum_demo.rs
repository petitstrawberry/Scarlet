#[cfg(target_os = "scarlet")]
use std::io;
#[cfg(target_os = "scarlet")]
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
#[cfg(target_os = "scarlet")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(target_os = "scarlet")]
use std::time::Duration;

#[cfg(target_os = "scarlet")]
use axum::extract::{ConnectInfo, Path};
#[cfg(target_os = "scarlet")]
use axum::http::HeaderMap;
#[cfg(target_os = "scarlet")]
use axum::{Router, routing::get};
#[cfg(target_os = "scarlet")]
use tokio::net::TcpListener;
#[cfg(target_os = "scarlet")]
use tokio::time::sleep;

#[cfg(target_os = "scarlet")]
const LISTEN_PORT: u16 = 8080;
#[cfg(target_os = "scarlet")]
const MAX_DELAY_MILLISECONDS: u64 = 10_000;
#[cfg(target_os = "scarlet")]
const REQUEST_ID_HEADER: &str = "x-scarlet-request-id";
#[cfg(target_os = "scarlet")]
static NEXT_REQUEST_ID: AtomicUsize = AtomicUsize::new(1);

#[cfg(target_os = "scarlet")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let app = Router::new()
        .route("/", get(root))
        .route("/delay/{milliseconds}", get(delay));
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, LISTEN_PORT));
    let listener = TcpListener::bind(address).await?;

    println!("Axum listening on http://{}", listener.local_addr()?);
    println!("Concurrency endpoint: GET /delay/{{milliseconds}}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

#[cfg(target_os = "scarlet")]
async fn root() -> &'static str {
    "Hello from Axum on Scarlet!\n"
}

#[cfg(target_os = "scarlet")]
async fn delay(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(milliseconds): Path<u64>,
    headers: HeaderMap,
) -> String {
    let server_request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let client_request_id = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-");
    let milliseconds = milliseconds.min(MAX_DELAY_MILLISECONDS);

    println!(
        "[axum-demo] start server={server_request_id} client={client_request_id} peer={peer} delay={milliseconds}ms"
    );
    sleep(Duration::from_millis(milliseconds)).await;
    println!(
        "[axum-demo] finish server={server_request_id} client={client_request_id} peer={peer}"
    );

    format!(
        "server-request={server_request_id} client-request={client_request_id} peer={peer} delay={milliseconds}ms\n"
    )
}

#[cfg(not(target_os = "scarlet"))]
fn main() {
    eprintln!("axum-demo is only available on Scarlet");
}
