use std::error::Error;
use std::time::{Duration, Instant};

use scarlet_tonic_demo::DEFAULT_ENDPOINT;
use scarlet_tonic_demo::demo::demo_client::DemoClient;
use scarlet_tonic_demo::demo::{CountdownRequest, GreetRequest};
use tokio::task::JoinSet;
use tonic::Request;

const UNARY_DELAYS_MILLISECONDS: &[u64] = &[1_200, 900, 600, 300];
const COUNTDOWN_FROM: u32 = 5;
const COUNTDOWN_INTERVAL_MILLISECONDS: u64 = 300;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let endpoint = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());

    println!("[tonic-client] connecting to {endpoint}");
    let client = DemoClient::connect(endpoint).await?;
    run_concurrent_unary_calls(client.clone()).await?;
    run_server_stream(client).await?;
    Ok(())
}

async fn run_concurrent_unary_calls(
    client: DemoClient<tonic::transport::Channel>,
) -> Result<(), tonic::Status> {
    println!(
        "[tonic-client] launching {} unary RPCs over one HTTP/2 channel",
        UNARY_DELAYS_MILLISECONDS.len()
    );
    let wall_started = Instant::now();
    let mut requests = JoinSet::new();

    for (index, delay_milliseconds) in UNARY_DELAYS_MILLISECONDS.iter().copied().enumerate() {
        let request_id = (index + 1) as u32;
        let mut task_client = client.clone();
        requests.spawn(async move {
            let started = Instant::now();
            let reply = task_client
                .greet(Request::new(GreetRequest {
                    request_id,
                    name: format!("task-{request_id}"),
                    delay_milliseconds,
                }))
                .await?
                .into_inner();
            Ok::<_, tonic::Status>((reply, started.elapsed()))
        });
    }

    let mut completed = 0usize;
    let mut accumulated_request_time = Duration::ZERO;
    while let Some(result) = requests.join_next().await {
        let (reply, elapsed) = result
            .map_err(|error| tonic::Status::internal(format!("RPC task failed: {error}")))??;
        completed += 1;
        accumulated_request_time += elapsed;
        println!(
            "[tonic-client] done={completed} request={} delay={}ms elapsed={}ms message={}",
            reply.request_id,
            reply.delay_milliseconds,
            elapsed.as_millis(),
            reply.message
        );
    }

    println!(
        "[tonic-client] unary wall={}ms accumulated-request-time={}ms",
        wall_started.elapsed().as_millis(),
        accumulated_request_time.as_millis()
    );

    Ok(())
}

async fn run_server_stream(
    mut client: DemoClient<tonic::transport::Channel>,
) -> Result<(), tonic::Status> {
    println!(
        "[tonic-client] starting server stream from={COUNTDOWN_FROM} interval={COUNTDOWN_INTERVAL_MILLISECONDS}ms"
    );
    let mut stream = client
        .countdown(Request::new(CountdownRequest {
            from: COUNTDOWN_FROM,
            interval_milliseconds: COUNTDOWN_INTERVAL_MILLISECONDS,
        }))
        .await?
        .into_inner();

    while let Some(reply) = stream.message().await? {
        println!("[tonic-client] stream value={}", reply.value);
    }
    println!("[tonic-client] stream complete");
    Ok(())
}
