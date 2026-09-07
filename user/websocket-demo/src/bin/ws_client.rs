use std::error::Error;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use scarlet_websocket_demo::DEFAULT_ENDPOINT;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const MESSAGES: &[&str] = &["hello", "is this thing on?", "goodbye"];
const SEND_INTERVAL: Duration = Duration::from_millis(500);

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let endpoint = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());

    println!("[ws-client] connecting to {endpoint}");
    let (websocket, response) = connect_async(&endpoint).await?;
    println!("[ws-client] connected, HTTP status={}", response.status());

    let (mut sink, mut stream) = websocket.split();

    let reader = tokio::spawn(async move {
        while let Some(result) = stream.next().await {
            match result {
                Ok(Message::Text(text)) => println!("[ws-client] received: {text}"),
                Ok(Message::Close(_)) => {
                    println!("[ws-client] server closed connection");
                    break;
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("[ws-client] receive error: {error}");
                    break;
                }
            }
        }
    });

    for (index, message) in MESSAGES.iter().enumerate() {
        sleep(SEND_INTERVAL).await;
        let payload = format!("{message} (#{})", index + 1);
        println!("[ws-client] sending: {payload}");
        sink.send(Message::Text(payload.into())).await?;
    }

    sleep(Duration::from_secs(2)).await;
    sink.send(Message::Close(None)).await?;
    let _ = reader.await;
    println!("[ws-client] done");
    Ok(())
}
