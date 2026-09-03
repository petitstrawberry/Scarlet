use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::{SinkExt, StreamExt};
use scarlet_websocket_demo::{BROADCAST_CAPACITY, DEFAULT_LISTEN_ADDRESS};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

static NEXT_CLIENT_ID: AtomicUsize = AtomicUsize::new(1);

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_LISTEN_ADDRESS.to_owned())
        .parse::<SocketAddr>()?;
    let listener = TcpListener::bind(address).await?;
    println!("[ws-server] listening on ws://{address}");

    let (broadcast_sender, _) = broadcast::channel::<String>(BROADCAST_CAPACITY);

    loop {
        let (stream, peer) = listener.accept().await?;
        let sender = broadcast_sender.clone();
        println!("[ws-server] TCP accepted from {peer}");
        tokio::spawn(async move {
            if let Err(error) = handle_client(stream, sender).await {
                eprintln!("[ws-server] client error: {error}");
            }
        });
    }
}

async fn handle_client(
    stream: TcpStream,
    broadcast_sender: broadcast::Sender<String>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let websocket = tokio_tungstenite::accept_async(stream).await?;
    let mut broadcast_receiver = broadcast_sender.subscribe();
    println!("[ws-server] client-{client_id} WebSocket established");

    let _ = broadcast_sender.send(format!("[system] client-{client_id} joined"));

    let (mut sink, mut stream) = websocket.split();
    loop {
        tokio::select! {
            inbound = stream.next() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        println!("[ws-server] client-{client_id}: {text}");
                        let _ = broadcast_sender.send(format!("client-{client_id}: {text}"));
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        println!("[ws-server] client-{client_id} disconnected");
                        let _ = broadcast_sender.send(format!("[system] client-{client_id} left"));
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        eprintln!("[ws-server] client-{client_id} receive error: {error}");
                        break;
                    }
                }
            }
            outbound = broadcast_receiver.recv() => {
                match outbound {
                    Ok(message) => {
                        sink.send(Message::Text(message.into())).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        eprintln!("[ws-server] client-{client_id} lagged by {count} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    let _ = sink.send(Message::Close(None)).await;
    Ok(())
}
