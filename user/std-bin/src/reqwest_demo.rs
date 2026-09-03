#[cfg(target_os = "scarlet")]
use std::error::Error;
#[cfg(target_os = "scarlet")]
use std::num::NonZeroU32;
#[cfg(target_os = "scarlet")]
use std::time::{Duration, Instant};

#[cfg(target_os = "scarlet")]
use clap::Parser;
#[cfg(target_os = "scarlet")]
use reqwest::header::CONTENT_TYPE;
#[cfg(target_os = "scarlet")]
use reqwest::{Client, StatusCode, Url, Version};
#[cfg(target_os = "scarlet")]
use tokio::task::JoinSet;

#[cfg(target_os = "scarlet")]
const DEFAULT_URLS: &[&str] = &[
    "https://example.com/",
    "https://www.rust-lang.org/",
    "https://docs.rs/reqwest/latest/reqwest/",
];
#[cfg(target_os = "scarlet")]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "scarlet")]
const BODY_PREVIEW_LIMIT: usize = 160;
#[cfg(target_os = "scarlet")]
const MAX_REQUESTS: usize = 32;
#[cfg(target_os = "scarlet")]
const CUSTOM_RANDOM_ERROR: u32 = getrandom::Error::CUSTOM_START + 1;

#[cfg(target_os = "scarlet")]
#[derive(Parser)]
#[command(about = "Fetch HTTP(S) URLs concurrently with reqwest on Scarlet")]
struct Arguments {
    /// Repeat every URL this many times.
    #[arg(short = 'n', long, default_value_t = 1)]
    repeat: usize,

    /// URLs to fetch. A small HTTPS batch is used when omitted.
    #[arg(value_name = "URL")]
    urls: Vec<String>,
}

#[cfg(target_os = "scarlet")]
struct FetchReport {
    request_id: usize,
    requested_url: Url,
    final_url: Url,
    status: StatusCode,
    version: Version,
    remote_address: Option<std::net::SocketAddr>,
    content_type: Option<String>,
    advertised_bytes: Option<u64>,
    received_bytes: usize,
    chunks: usize,
    preview: Vec<u8>,
    elapsed: Duration,
}

#[cfg(target_os = "scarlet")]
getrandom::register_custom_getrandom!(scarlet_getrandom);

#[cfg(target_os = "scarlet")]
fn scarlet_getrandom(destination: &mut [u8]) -> Result<(), getrandom::Error> {
    let mut offset = 0usize;
    while offset < destination.len() {
        let result = scarlet_sys::syscall3(
            scarlet_sys::Syscall::GetRandom,
            destination[offset..].as_mut_ptr() as usize,
            destination.len() - offset,
            scarlet_sys::GET_RANDOM_FLAG_REQUIRE_ENTROPY,
        );
        if result == usize::MAX || result == 0 || result > destination.len() - offset {
            let code = NonZeroU32::new(CUSTOM_RANDOM_ERROR)
                .expect("custom getrandom error code must be non-zero");
            return Err(getrandom::Error::from(code));
        }
        offset += result;
    }
    Ok(())
}

#[cfg(target_os = "scarlet")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let urls = build_request_urls(arguments)?;

    let client = Client::builder()
        .user_agent("scarlet-reqwest-demo/0.1")
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    println!(
        "[reqwest-demo] launching {} concurrent request(s)",
        urls.len()
    );
    let wall_started = Instant::now();
    let mut requests = JoinSet::new();
    for (index, url) in urls.into_iter().enumerate() {
        let request_id = index + 1;
        println!("[{request_id}] start {url}");
        requests.spawn(fetch(request_id, client.clone(), url));
    }

    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut accumulated_request_time = Duration::ZERO;
    while let Some(result) = requests.join_next().await {
        match result {
            Ok(Ok(report)) => {
                completed += 1;
                accumulated_request_time += report.elapsed;
                print_report(completed + failed, &report);
            }
            Ok(Err((request_id, url, error))) => {
                failed += 1;
                println!(
                    "[request {request_id}, done {}] failed {url}: {error}",
                    completed + failed
                );
            }
            Err(error) => {
                failed += 1;
                println!("[done {}] task failed: {error}", completed + failed);
            }
        }
    }

    let wall_elapsed = wall_started.elapsed();
    println!(
        "[reqwest-demo] completed={completed} failed={failed} wall={}ms accumulated-request-time={}ms",
        wall_elapsed.as_millis(),
        accumulated_request_time.as_millis()
    );

    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} request(s) failed").into())
    }
}

#[cfg(target_os = "scarlet")]
fn build_request_urls(arguments: Arguments) -> Result<Vec<Url>, Box<dyn Error>> {
    if arguments.repeat == 0 {
        return Err("--repeat must be at least 1".into());
    }

    let raw_urls = if arguments.urls.is_empty() {
        DEFAULT_URLS.iter().map(|url| (*url).to_owned()).collect()
    } else {
        arguments.urls
    };

    let request_count = raw_urls
        .len()
        .checked_mul(arguments.repeat)
        .ok_or("request count overflow")?;
    if request_count > MAX_REQUESTS {
        return Err(format!("at most {MAX_REQUESTS} requests may be launched at once").into());
    }

    let mut urls = Vec::with_capacity(request_count);
    for raw_url in raw_urls {
        let url = raw_url.parse::<Url>()?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("unsupported URL scheme in {url}").into());
        }
        for _ in 0..arguments.repeat {
            urls.push(url.clone());
        }
    }
    Ok(urls)
}

#[cfg(target_os = "scarlet")]
async fn fetch(
    request_id: usize,
    client: Client,
    requested_url: Url,
) -> Result<FetchReport, (usize, Url, reqwest::Error)> {
    let started = Instant::now();
    let mut response = match client.get(requested_url.clone()).send().await {
        Ok(response) => response,
        Err(error) => return Err((request_id, requested_url, error)),
    };

    let final_url = response.url().clone();
    let status = response.status();
    let version = response.version();
    let remote_address = response.remote_addr();
    let advertised_bytes = response.content_length();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let mut received_bytes = 0usize;
    let mut chunks = 0usize;
    let mut preview = Vec::with_capacity(BODY_PREVIEW_LIMIT);
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => return Err((request_id, requested_url, error)),
        };
        chunks += 1;
        received_bytes += chunk.len();
        let preview_remaining = BODY_PREVIEW_LIMIT.saturating_sub(preview.len());
        preview.extend_from_slice(&chunk[..chunk.len().min(preview_remaining)]);
    }

    Ok(FetchReport {
        request_id,
        requested_url,
        final_url,
        status,
        version,
        remote_address,
        content_type,
        advertised_bytes,
        received_bytes,
        chunks,
        preview,
        elapsed: started.elapsed(),
    })
}

#[cfg(target_os = "scarlet")]
fn print_report(completion_order: usize, report: &FetchReport) {
    println!(
        "[request {}, done {completion_order}] {} {:?} bytes={} chunks={} elapsed={}ms",
        report.request_id,
        report.status,
        report.version,
        report.received_bytes,
        report.chunks,
        report.elapsed.as_millis()
    );
    println!("  requested={}", report.requested_url);
    if report.final_url != report.requested_url {
        println!("  redirected={}", report.final_url);
    }
    if let Some(remote_address) = report.remote_address {
        println!("  peer={remote_address}");
    }
    if let Some(content_type) = &report.content_type {
        println!("  content-type={content_type}");
    }
    if let Some(advertised_bytes) = report.advertised_bytes {
        println!("  content-length={advertised_bytes}");
    }
    if is_textual(report.content_type.as_deref()) && !report.preview.is_empty() {
        println!("  preview={}", printable_preview(&report.preview));
    }
}

#[cfg(target_os = "scarlet")]
fn is_textual(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|content_type| {
        content_type.starts_with("text/")
            || content_type.contains("json")
            || content_type.contains("javascript")
            || content_type.contains("xml")
    })
}

#[cfg(target_os = "scarlet")]
fn printable_preview(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

#[cfg(not(target_os = "scarlet"))]
fn main() {
    eprintln!("reqwest-demo is only available on Scarlet");
}
