#[cfg(target_os = "scarlet")]
use std::env;
#[cfg(target_os = "scarlet")]
use std::error::Error;
#[cfg(target_os = "scarlet")]
use std::num::NonZeroU32;
#[cfg(target_os = "scarlet")]
use std::time::Duration;

#[cfg(target_os = "scarlet")]
use reqwest::{Client, Url};

#[cfg(target_os = "scarlet")]
const DEFAULT_URL: &str = "https://example.com/";
#[cfg(target_os = "scarlet")]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "scarlet")]
const BODY_PREVIEW_LIMIT: usize = 512;
#[cfg(target_os = "scarlet")]
const CUSTOM_RANDOM_ERROR: u32 = getrandom::Error::CUSTOM_START + 1;

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
    let url = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_URL.to_owned())
        .parse::<Url>()?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err("reqwest-demo supports only http:// and https:// URLs".into());
    }

    let client = Client::builder()
        .user_agent("scarlet-reqwest-demo/0.1")
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    println!("[reqwest-demo] GET {url}");
    let response = client.get(url).send().await?;

    println!("[reqwest-demo] status={}", response.status());
    for (name, value) in response.headers() {
        println!(
            "[reqwest-demo] header {name}: {}",
            String::from_utf8_lossy(value.as_bytes())
        );
    }

    let body = response.bytes().await?;
    println!("[reqwest-demo] body-bytes={}", body.len());
    if !body.is_empty() {
        let preview_len = body.len().min(BODY_PREVIEW_LIMIT);
        println!("[reqwest-demo] body-preview:");
        println!("{}", String::from_utf8_lossy(&body[..preview_len]));
    }

    Ok(())
}

#[cfg(not(target_os = "scarlet"))]
fn main() {
    eprintln!("reqwest-demo is only available on Scarlet");
}
