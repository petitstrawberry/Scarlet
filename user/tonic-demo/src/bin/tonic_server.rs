use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;

use scarlet_tonic_demo::DEFAULT_LISTEN_ADDRESS;
use scarlet_tonic_demo::demo::demo_server::{Demo, DemoServer};
use scarlet_tonic_demo::demo::{CountdownReply, CountdownRequest, GreetReply, GreetRequest};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

const MAX_UNARY_DELAY_MILLISECONDS: u64 = 10_000;
const MAX_COUNTDOWN_VALUE: u32 = 20;
const MAX_STREAM_INTERVAL_MILLISECONDS: u64 = 2_000;

#[derive(Default)]
struct DemoService;

#[tonic::async_trait]
impl Demo for DemoService {
    async fn greet(&self, request: Request<GreetRequest>) -> Result<Response<GreetReply>, Status> {
        let request = request.into_inner();
        let delay_milliseconds = request.delay_milliseconds.min(MAX_UNARY_DELAY_MILLISECONDS);

        println!(
            "[tonic-server] start request={} name={} delay={}ms",
            request.request_id, request.name, delay_milliseconds
        );
        sleep(Duration::from_millis(delay_milliseconds)).await;
        println!("[tonic-server] finish request={}", request.request_id);

        Ok(Response::new(GreetReply {
            request_id: request.request_id,
            message: format!("Hello, {} from Tonic on Scarlet!", request.name),
            delay_milliseconds,
        }))
    }

    type CountdownStream = ReceiverStream<Result<CountdownReply, Status>>;

    async fn countdown(
        &self,
        request: Request<CountdownRequest>,
    ) -> Result<Response<Self::CountdownStream>, Status> {
        let request = request.into_inner();
        let from = request.from.min(MAX_COUNTDOWN_VALUE);
        let interval_milliseconds = request
            .interval_milliseconds
            .min(MAX_STREAM_INTERVAL_MILLISECONDS);
        let (sender, receiver) = mpsc::channel(4);

        println!("[tonic-server] countdown from={from} interval={interval_milliseconds}ms");
        tokio::spawn(async move {
            for value in (0..=from).rev() {
                if sender.send(Ok(CountdownReply { value })).await.is_err() {
                    println!("[tonic-server] countdown client disconnected");
                    return;
                }
                if value != 0 {
                    sleep(Duration::from_millis(interval_milliseconds)).await;
                }
            }
            println!("[tonic-server] countdown finished");
        });

        Ok(Response::new(ReceiverStream::new(receiver)))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_LISTEN_ADDRESS.to_owned())
        .parse::<SocketAddr>()?;

    println!("[tonic-server] listening on http://{address}");
    Server::builder()
        .add_service(DemoServer::new(DemoService))
        .serve(address)
        .await?;
    Ok(())
}
