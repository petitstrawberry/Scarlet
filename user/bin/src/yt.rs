#![no_std]
#![no_main]

extern crate alloc;
extern crate getrandom;
extern crate rustls;
extern crate scarlet_std as std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::num::NonZeroU32;
use core::time::Duration;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::time_provider::TimeProvider;
use rustls::unbuffered::ConnectionState;
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use std::env;
use std::fs::{File, OpenOptions, remove_file};
use std::io::{ErrorKind, Read, Write, stdin, stdout};
use std::network::list_interfaces;
use std::poll::{POLLIN, PollHandle, poll};
use std::println;
use std::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};
use std::sync::Mutex;
use std::task::{execve, exit, fork, getpid, waitpid};
use std::thread;

const DEFAULT_DNS: [u8; 4] = [10, 0, 2, 3];
const DNS_PORT: u16 = 53;
const HTTP_PORT: u16 = 80;
const HTTPS_PORT: u16 = 443;
const DNS_TIMEOUT_NS: i64 = 5_000_000_000;
const HTTP_TIMEOUT_NS: i64 = 10_000_000_000;
const TLS_TIMEOUT_NS: i64 = 60_000_000_000;
const MAX_REDIRECTS: usize = 8;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTPS_RESPONSE_BYTES: usize = 512 * 1024 * 1024;
const GETRANDOM_ERROR: u32 = getrandom::Error::CUSTOM_START + 1;
const LOG_TLS_IO: bool = false;
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 Scarlet-yt/0.1";
const DEFAULT_EXTRA_HEADERS: &str = "";
const YOUTUBE_MEDIA_EXTRA_HEADERS: &str =
    "Accept-Language: en-US,en;q=0.9\r\nRange: bytes=0-\r\nReferer: https://www.youtube.com/\r\n";
const YOUTUBE_WEB_CLIENT_VERSION: &str = "2.20260114.08.00";
const YOUTUBE_MWEB_CLIENT_VERSION: &str = "2.20260115.01.00";
const YOUTUBE_ANDROID_CLIENT_VERSION: &str = "21.02.35";
const YOUTUBE_ANDROID_VR_CLIENT_VERSION: &str = "1.65.10";
const YOUTUBE_IOS_CLIENT_VERSION: &str = "21.02.3";

getrandom::register_custom_getrandom!(scarlet_getrandom);

fn scarlet_getrandom(dest: &mut [u8]) -> Result<(), getrandom::Error> {
    let mut file = File::open("/dev/random").map_err(|_| custom_getrandom_error())?;
    let mut offset = 0usize;
    while offset < dest.len() {
        let n = file
            .read(&mut dest[offset..])
            .map_err(|_| custom_getrandom_error())?;
        if n == 0 {
            return Err(custom_getrandom_error());
        }
        offset += n;
    }
    Ok(())
}

fn custom_getrandom_error() -> getrandom::Error {
    getrandom::Error::from(NonZeroU32::new(GETRANDOM_ERROR).unwrap())
}

fn fallback_youtube_innertube_api_key() -> String {
    let mut key = String::new();
    for part in [
        "AI",
        "za",
        "Sy",
        "AO",
        "_FJ2SlqU8Q4STEHLGCilw",
        "_Y9_11qcW8",
    ] {
        key.push_str(part);
    }
    key
}

#[derive(Clone, Debug)]
struct UrlParts {
    scheme: String,
    host: String,
    port: u16,
    path: String,
}

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    match run() {
        Ok(()) => 0,
        Err(error) => {
            println!("[yt] error: {}", error);
            1
        }
    }
}

fn run() -> Result<(), String> {
    let args = env::args_vec();
    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    let mut output: Option<String> = None;
    let mut print_headers = false;
    let mut play = true;
    let mut positional = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    return Err(String::from("--output requires a path"));
                }
                output = Some(args[i].clone());
            }
            "--headers" => {
                print_headers = true;
            }
            "--no-play" => {
                play = false;
            }
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            value => positional.push(value.to_string()),
        }
        i += 1;
    }

    let mut input = parse_input_or_search_query(&positional)?;
    if !is_youtube_url(&input) && !looks_like_url(&input) {
        input = select_youtube_search_result(&input)?.watch_url();
    }
    let mut media = MediaSelection {
        video_url: input.clone(),
        audio_url: None,
        user_agent: DEFAULT_USER_AGENT,
        extra_headers: DEFAULT_EXTRA_HEADERS,
    };
    if is_youtube_url(&input) {
        println!("[yt] YouTube URL detected");
        let video_id =
            youtube_video_id(&input).ok_or_else(|| String::from("YouTube video id not found"))?;
        println!("[yt] video id: {}", video_id);
        media = resolve_youtube_media(&input)?;
        if output.is_none() {
            output = Some(format!("/root/media/youtube-{}.mp4", video_id));
        }
    }

    let output = output.unwrap_or_else(|| String::from("/tmp/yt.mp4"));
    if play {
        if let Some(audio_url) = media.audio_url {
            let audio_output = derive_audio_output_path(&output);
            return stream_media_pair_and_play(
                media.video_url,
                output,
                audio_url,
                audio_output,
                print_headers,
                media.user_agent,
                media.extra_headers,
            );
        }
    }

    let audio_output = if let Some(audio_url) = media.audio_url {
        let path = derive_audio_output_path(&output);
        fetch_media_pair_to_files(
            media.video_url,
            output.clone(),
            audio_url,
            path.clone(),
            print_headers,
            media.user_agent,
            media.extra_headers,
        )?;
        Some(path)
    } else {
        fetch_url_to_file(
            &media.video_url,
            &output,
            print_headers,
            media.user_agent,
            media.extra_headers,
        )?;
        println!("[yt] saved {}", output);
        None
    };
    if play {
        let rc = if let Some(audio) = audio_output.as_ref() {
            println!(
                "[yt] exec: video_player --hwdc {} --audio {}",
                output, audio
            );
            let argv = [
                "video_player",
                "--hwdc",
                output.as_str(),
                "--audio",
                audio.as_str(),
            ];
            execve("/bin/video_player", &argv, &[])
        } else {
            println!("[yt] exec: video_player --hwdc {}", output);
            let argv = ["video_player", "--hwdc", output.as_str()];
            execve("/bin/video_player", &argv, &[])
        };
        return Err(format!("failed to exec /bin/video_player: {}", rc));
    }
    if let Some(audio) = audio_output {
        println!(
            "[yt] play: video_player --hwdc {} --audio {}",
            output, audio
        );
    } else {
        println!("[yt] play: video_player --hwdc {}", output);
    }
    Ok(())
}

fn parse_input_or_search_query(positional: &[String]) -> Result<String, String> {
    if positional.is_empty() {
        return Err(String::from("missing URL or search query"));
    }
    if positional.first().map(String::as_str) == Some("search") {
        return join_query_terms(&positional[1..]);
    }
    join_query_terms(positional)
}

fn join_query_terms(terms: &[String]) -> Result<String, String> {
    if terms.is_empty() {
        return Err(String::from("missing search query"));
    }
    let mut out = String::new();
    for term in terms {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(term);
    }
    Ok(out)
}

fn stream_media_pair_and_play(
    video_url: String,
    video_output: String,
    audio_url: String,
    audio_output: String,
    print_headers: bool,
    user_agent: &'static str,
    extra_headers: &'static str,
) -> Result<(), String> {
    let video_socket_path = stream_socket_path(&video_output, "video");
    let stream_audio_output = stream_file_path(&audio_output, "audio");

    println!("[yt] downloading DASH audio before playback");
    let _ = remove_file(&stream_audio_output);
    fetch_url_to_file_streaming(
        &audio_url,
        &stream_audio_output,
        print_headers,
        user_agent,
        extra_headers,
    )?;
    println!("[yt] saved {}", stream_audio_output);

    let video_listener = prepare_stream_socket(&video_socket_path)?;

    let child = fork();
    if child < 0 {
        return Err(String::from("failed to fork video_player"));
    }
    if child == 0 {
        println!(
            "[yt] exec: video_player --hwdc --stream-socket {} --audio {}",
            video_socket_path, stream_audio_output
        );
        let argv = [
            "video_player",
            "--hwdc",
            "--stream",
            "--stream-socket",
            video_socket_path.as_str(),
            "--audio",
            stream_audio_output.as_str(),
            video_output.as_str(),
        ];
        let rc = execve("/bin/video_player", &argv, &[]);
        println!("[yt] failed to exec /bin/video_player: {}", rc);
        exit(1);
    }

    let download_result = match accept_stream_socket(&video_listener, &video_socket_path) {
        Ok(video_stream) => {
            println!("[yt] streaming DASH video");
            fetch_url_to_socket_streaming(
                &video_url,
                video_stream,
                print_headers,
                user_agent,
                extra_headers,
            )
        }
        Err(err) => Err(err),
    };
    let _ = remove_file(&video_socket_path);

    let (_pid, status) = waitpid(child, 0);
    if let Err(error) = download_result {
        if status == 0 && is_media_stream_closed_by_player(&error) {
            println!("[yt] video_player closed; stopped media stream");
            return Ok(());
        }
        return Err(error);
    }
    if status != 0 {
        return Err(format!("video_player exited with status {}", status));
    }
    Ok(())
}

fn is_media_stream_closed_by_player(error: &str) -> bool {
    error.starts_with("media stream write failed") || error.starts_with("media stream short write")
}

fn fetch_media_pair_to_files(
    video_url: String,
    video_output: String,
    audio_url: String,
    audio_output: String,
    print_headers: bool,
    user_agent: &'static str,
    extra_headers: &'static str,
) -> Result<(), String> {
    fetch_media_pair_to_files_with_markers(
        video_url,
        video_output,
        None,
        audio_url,
        audio_output,
        None,
        print_headers,
        user_agent,
        extra_headers,
    )
}

fn fetch_media_pair_to_files_with_markers(
    video_url: String,
    video_output: String,
    video_complete: Option<String>,
    audio_url: String,
    audio_output: String,
    audio_complete: Option<String>,
    print_headers: bool,
    user_agent: &'static str,
    extra_headers: &'static str,
) -> Result<(), String> {
    println!("[yt] downloading DASH video/audio concurrently");
    let video_result = Arc::new(Mutex::new(None));
    let audio_result = Arc::new(Mutex::new(None));

    let video_result_writer = video_result.clone();
    let video_output_writer = video_output.clone();
    let video_handle = thread::spawn(move || {
        let result = fetch_url_to_file_streaming(
            &video_url,
            &video_output_writer,
            print_headers,
            user_agent,
            extra_headers,
        );
        if result.is_ok() {
            println!("[yt] saved {}", video_output_writer);
            if let Some(marker) = video_complete {
                if let Err(err) = touch_file(&marker) {
                    *video_result_writer.lock() = Some(Err(err));
                    return;
                }
            }
        }
        *video_result_writer.lock() = Some(result);
    });

    let audio_result_writer = audio_result.clone();
    let audio_output_writer = audio_output.clone();
    let audio_handle = thread::spawn(move || {
        let result = fetch_url_to_file_streaming(
            &audio_url,
            &audio_output_writer,
            print_headers,
            user_agent,
            extra_headers,
        );
        if result.is_ok() {
            println!("[yt] saved {}", audio_output_writer);
            if let Some(marker) = audio_complete {
                if let Err(err) = touch_file(&marker) {
                    *audio_result_writer.lock() = Some(Err(err));
                    return;
                }
            }
        }
        *audio_result_writer.lock() = Some(result);
    });

    video_handle
        .join()
        .map_err(|err| format!("video download thread failed: {}", err))?;
    audio_handle
        .join()
        .map_err(|err| format!("audio download thread failed: {}", err))?;

    video_result
        .lock()
        .take()
        .unwrap_or_else(|| Err(String::from("video download did not finish")))?;
    audio_result
        .lock()
        .take()
        .unwrap_or_else(|| Err(String::from("audio download did not finish")))?;
    Ok(())
}

fn stream_socket_path(path: &str, kind: &str) -> String {
    format!(
        "/tmp/scarlet-yt-{}-{}-{}.sock",
        getpid(),
        kind,
        path_basename(path)
    )
}

fn stream_file_path(path: &str, kind: &str) -> String {
    format!(
        "/tmp/scarlet-yt-{}-{}-{}",
        getpid(),
        kind,
        path_basename(path)
    )
}

fn path_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn prepare_stream_socket(path: &str) -> Result<Socket, String> {
    let _ = remove_file(path);
    let socket = Socket::new().map_err(|_| format!("failed to create local socket {path}"))?;
    socket
        .bind(path)
        .map_err(|_| format!("failed to bind local socket {path}"))?;
    socket
        .listen(1)
        .map_err(|_| format!("failed to listen on local socket {path}"))?;
    Ok(socket)
}

fn accept_stream_socket(listener: &Socket, path: &str) -> Result<Socket, String> {
    for _ in 0..400 {
        match listener.accept() {
            Ok(socket) => return Ok(socket),
            Err(_) => {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    Err(format!(
        "timed out waiting for video_player to connect {path}"
    ))
}

fn touch_file(path: &str) -> Result<(), String> {
    let mut options = OpenOptions::new();
    let _file = options
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|_| format!("failed to create {}", path))?;
    Ok(())
}

fn print_usage() {
    println!("Usage: yt [options] URL");
    println!("       yt [options] search QUERY");
    println!("       yt [options] QUERY");
    println!();
    println!("Options:");
    println!("  -o, --output <path>  Save response body");
    println!("  --headers           Print response headers");
    println!("  --no-play           Download only");
    println!("  -h, --help          Show this help");
    println!();
    println!("Examples:");
    println!("  yt 'https://www.youtube.com/watch?v=...'");
    println!("  yt search hanaarashi eve");
    println!("  yt --no-play -o /root/media/video.mp4 'https://example.com/video.mp4'");
}

struct YoutubeSearchResult {
    video_id: String,
    title: String,
    channel: Option<String>,
    duration: Option<String>,
}

impl YoutubeSearchResult {
    fn watch_url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.video_id)
    }
}

fn select_youtube_search_result(query: &str) -> Result<YoutubeSearchResult, String> {
    let results = youtube_search(query)?;
    if results.is_empty() {
        return Err(String::from("no YouTube search results found"));
    }

    println!("[yt] search results for '{}':", query);
    for (index, result) in results.iter().take(10).enumerate() {
        let channel = result.channel.as_deref().unwrap_or("unknown");
        let duration = result.duration.as_deref().unwrap_or("--:--");
        println!(
            "[{}] {} [{}] - {}",
            index + 1,
            result.title,
            duration,
            channel
        );
    }

    let prompt = "select video [1-10, enter=1, q=cancel]: ";
    let choice = read_prompt_line(prompt)?;
    if choice.is_empty() {
        return Ok(results.into_iter().next().unwrap());
    }
    if choice == "q" || choice == "Q" {
        return Err(String::from("selection canceled"));
    }
    let index = parse_usize(&choice).ok_or_else(|| String::from("invalid selection"))?;
    if index == 0 || index > results.len().min(10) {
        return Err(String::from("selection out of range"));
    }
    results
        .into_iter()
        .nth(index - 1)
        .ok_or_else(|| String::from("selection out of range"))
}

fn youtube_search(query: &str) -> Result<Vec<YoutubeSearchResult>, String> {
    let mut encoded_query = String::new();
    push_form_encoded(&mut encoded_query, query);
    let url = format!(
        "https://www.youtube.com/results?search_query={}",
        encoded_query
    );
    println!("[yt] searching YouTube: {}", query);
    let response = https_get(
        &parse_url(&url)?,
        DEFAULT_USER_AGENT,
        "Accept-Language: en-US,en;q=0.9\r\n",
    )?;
    if response.status < 200 || response.status >= 300 {
        return Err(format!("YouTube search HTTP status {}", response.status));
    }
    let page = core::str::from_utf8(&response.body)
        .map_err(|_| String::from("YouTube search page is not UTF-8"))?;
    Ok(parse_youtube_search_results(page))
}

fn parse_youtube_search_results(page: &str) -> Vec<YoutubeSearchResult> {
    let mut results = Vec::new();
    let mut offset = 0usize;
    let pattern = "\"videoRenderer\":";

    while results.len() < 20 {
        let Some(found) = page[offset..].find(pattern) else {
            break;
        };
        let mut object_start = offset + found + pattern.len();
        while page
            .as_bytes()
            .get(object_start)
            .copied()
            .map(|byte| byte.is_ascii_whitespace())
            .unwrap_or(false)
        {
            object_start += 1;
        }
        let Some(object_end) = find_matching_json(page, object_start, b'{', b'}') else {
            break;
        };
        let object = &page[object_start..=object_end];
        if let Some(result) = parse_youtube_search_result(object)
            && !results
                .iter()
                .any(|existing: &YoutubeSearchResult| existing.video_id == result.video_id)
        {
            results.push(result);
        }
        offset = object_end + 1;
    }

    results
}

fn parse_youtube_search_result(object: &str) -> Option<YoutubeSearchResult> {
    let video_id = json_string_field(object, "videoId")?;
    let title = youtube_renderer_text(object, "title")?;
    Some(YoutubeSearchResult {
        video_id,
        title,
        channel: youtube_renderer_text(object, "ownerText")
            .or_else(|| youtube_renderer_text(object, "longBylineText"))
            .or_else(|| youtube_renderer_text(object, "shortBylineText")),
        duration: youtube_renderer_text(object, "lengthText"),
    })
}

fn youtube_renderer_text(object: &str, field: &str) -> Option<String> {
    let value = json_object_field(object, field)?;
    json_string_field(value, "simpleText").or_else(|| json_string_field(value, "text"))
}

fn json_object_field<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("\"{}\":{{", name);
    let object_start = input.find(&pattern)? + pattern.len() - 1;
    let object_end = find_matching_json(input, object_start, b'{', b'}')?;
    Some(&input[object_start..=object_end])
}

fn read_prompt_line(prompt: &str) -> Result<String, String> {
    let out = stdout();
    out.write_all(prompt.as_bytes())
        .map_err(|_| String::from("failed to write prompt"))?;
    out.flush()
        .map_err(|_| String::from("failed to flush prompt"))?;

    let input = stdin();
    let mut line = String::new();
    let mut buffer = [0u8; 64];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|_| String::from("failed to read selection"))?;
        if read == 0 {
            continue;
        }
        for byte in &buffer[..read] {
            match *byte {
                b'\n' | b'\r' => return Ok(line.trim().to_string()),
                byte => line.push(byte as char),
            }
        }
    }
}

fn fetch_url_to_file_streaming(
    url: &str,
    output: &str,
    print_headers: bool,
    user_agent: &str,
    extra_headers: &str,
) -> Result<(), String> {
    let mut current = parse_url(url)?;
    for redirect_index in 0..=MAX_REDIRECTS {
        println!(
            "[yt] GET {}://{}:{}{}",
            current.scheme,
            current.host,
            current.port,
            display_path(&current.path)
        );
        let response = match current.scheme.as_str() {
            "https" => https_get_to_file(&current, output, user_agent, extra_headers)?,
            _ => {
                fetch_url_to_file(url, output, print_headers, user_agent, extra_headers)?;
                return Ok(());
            }
        };
        if print_headers {
            println!("{}", response.headers);
        }

        if is_redirect(response.status) {
            let location = header_value(&response.headers, "location")
                .ok_or_else(|| String::from("redirect without Location header"))?;
            if redirect_index == MAX_REDIRECTS {
                return Err(String::from("too many redirects"));
            }
            current = resolve_redirect(&current, &location)?;
            continue;
        }

        if response.status < 200 || response.status >= 300 {
            print_http_error_body(response.status, &response.body);
            return Err(format!("HTTP status {}", response.status));
        }

        return Ok(());
    }

    Err(String::from("too many redirects"))
}

fn fetch_url_to_socket_streaming(
    url: &str,
    mut output: Socket,
    print_headers: bool,
    user_agent: &str,
    extra_headers: &str,
) -> Result<(), String> {
    let mut current = parse_url(url)?;
    for redirect_index in 0..=MAX_REDIRECTS {
        println!(
            "[yt] GET {}://{}:{}{}",
            current.scheme,
            current.host,
            current.port,
            display_path(&current.path)
        );
        let response = match current.scheme.as_str() {
            "https" => https_get_to_socket(&current, &mut output, user_agent, extra_headers)?,
            _ => {
                let response = http_get(&current, user_agent, extra_headers)?;
                write_all(&mut output, &response.body, "media stream")?;
                response
            }
        };
        if print_headers {
            println!("{}", response.headers);
        }

        if is_redirect(response.status) {
            let location = header_value(&response.headers, "location")
                .ok_or_else(|| String::from("redirect without Location header"))?;
            if redirect_index == MAX_REDIRECTS {
                return Err(String::from("too many redirects"));
            }
            current = resolve_redirect(&current, &location)?;
            continue;
        }

        if response.status < 200 || response.status >= 300 {
            print_http_error_body(response.status, &response.body);
            return Err(format!("HTTP status {}", response.status));
        }

        return Ok(());
    }

    Err(String::from("too many redirects"))
}

fn fetch_url_to_file(
    url: &str,
    output: &str,
    print_headers: bool,
    user_agent: &str,
    extra_headers: &str,
) -> Result<(), String> {
    let mut current = parse_url(url)?;
    for redirect_index in 0..=MAX_REDIRECTS {
        println!(
            "[yt] GET {}://{}:{}{}",
            current.scheme,
            current.host,
            current.port,
            display_path(&current.path)
        );
        let response = http_get(&current, user_agent, extra_headers)?;
        if print_headers {
            println!("{}", response.headers);
        }

        if is_redirect(response.status) {
            let location = header_value(&response.headers, "location")
                .ok_or_else(|| String::from("redirect without Location header"))?;
            if redirect_index == MAX_REDIRECTS {
                return Err(String::from("too many redirects"));
            }
            current = resolve_redirect(&current, &location)?;
            continue;
        }

        if response.status < 200 || response.status >= 300 {
            print_http_error_body(response.status, &response.body);
            return Err(format!("HTTP status {}", response.status));
        }

        write_body(output, response.body)?;
        return Ok(());
    }

    Err(String::from("too many redirects"))
}

struct HttpResponse {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

struct MediaSelection {
    video_url: String,
    audio_url: Option<String>,
    user_agent: &'static str,
    extra_headers: &'static str,
}

fn http_get(url: &UrlParts, user_agent: &str, extra_headers: &str) -> Result<HttpResponse, String> {
    match url.scheme.as_str() {
        "http" => plain_http_get(url, user_agent, extra_headers),
        "https" => https_get(url, user_agent, extra_headers),
        _ => Err(format!("unsupported URL scheme: {}", url.scheme)),
    }
}

fn plain_http_get(
    url: &UrlParts,
    user_agent: &str,
    extra_headers: &str,
) -> Result<HttpResponse, String> {
    let ip = resolve_host(&url.host)?;
    println!(
        "[yt] resolved {} -> {}.{}.{}.{}",
        url.host, ip[0], ip[1], ip[2], ip[3]
    );

    let mut socket =
        Socket::new_with_domain(SocketDomain::Inet4, SocketType::Stream, SocketProtocol::Tcp)
            .map_err(|_| String::from("failed to create TCP socket"))?;
    socket
        .connect_inet(Inet4SocketAddress::new(ip, url.port))
        .map_err(|_| String::from("TCP connect failed"))?;
    println!("[yt] TCP connected {}:{}", url.host, url.port);

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\n{}Connection: close\r\n\r\n",
        url.path, url.host, user_agent, extra_headers
    );
    write_all(&mut socket, request.as_bytes(), "HTTP request")?;

    let mut received = Vec::new();
    let header_end = loop {
        if received.len() > MAX_HEADER_BYTES {
            return Err(String::from("HTTP headers are too large"));
        }
        if let Some(pos) = find_bytes(&received, b"\r\n\r\n") {
            break pos + 4;
        }
        wait_readable(&socket, HTTP_TIMEOUT_NS)?;
        let mut chunk = [0u8; 2048];
        let n = socket
            .read(&mut chunk)
            .map_err(|_| String::from("HTTP read failed"))?;
        if n == 0 {
            return Err(String::from("connection closed before headers"));
        }
        received.extend_from_slice(&chunk[..n]);
    };

    let headers = core::str::from_utf8(&received[..header_end])
        .map_err(|_| String::from("HTTP headers are not UTF-8"))?
        .to_string();
    let status = parse_status(&headers)?;
    let mut body = Vec::new();
    body.extend_from_slice(&received[header_end..]);

    if header_value(&headers, "transfer-encoding")
        .map(|value| value.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
    {
        read_chunked_body(&mut socket, &mut body)?;
    } else if let Some(length) = header_value(&headers, "content-length") {
        let content_length = parse_usize(length.trim())
            .ok_or_else(|| String::from("invalid Content-Length header"))?;
        while body.len() < content_length {
            wait_readable(&socket, HTTP_TIMEOUT_NS)?;
            let mut chunk = [0u8; 8192];
            let n = socket
                .read(&mut chunk)
                .map_err(|_| String::from("HTTP body read failed"))?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(content_length);
    } else {
        loop {
            let mut poll_handle = [PollHandle::new(socket.as_raw() as u32, POLLIN)];
            match poll(&mut poll_handle, 500_000_000) {
                Ok(0) => break,
                Ok(_) => {
                    let mut chunk = [0u8; 8192];
                    let n = socket
                        .read(&mut chunk)
                        .map_err(|_| String::from("HTTP body read failed"))?;
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                Err(_) => return Err(String::from("poll failed while reading HTTP body")),
            }
        }
    }

    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn https_get(
    url: &UrlParts,
    user_agent: &str,
    extra_headers: &str,
) -> Result<HttpResponse, String> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\n{}Connection: close\r\n\r\n",
        url.path, url.host, user_agent, extra_headers
    );
    https_request(url, request.as_bytes())
}

fn https_get_to_file(
    url: &UrlParts,
    output: &str,
    user_agent: &str,
    extra_headers: &str,
) -> Result<HttpResponse, String> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\n{}Connection: close\r\n\r\n",
        url.path, url.host, user_agent, extra_headers
    );
    https_request_to_file(url, request.as_bytes(), output)
}

fn https_get_to_socket(
    url: &UrlParts,
    output: &mut Socket,
    user_agent: &str,
    extra_headers: &str,
) -> Result<HttpResponse, String> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\n{}Connection: close\r\n\r\n",
        url.path, url.host, user_agent, extra_headers
    );
    https_request_to_socket(url, request.as_bytes(), output)
}

fn https_post_json(
    url: &UrlParts,
    body: &str,
    user_agent: &str,
    extra_headers: &str,
) -> Result<HttpResponse, String> {
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        url.path,
        url.host,
        user_agent,
        extra_headers,
        body.len(),
        body
    );
    https_request(url, request.as_bytes())
}

fn https_request_to_file(
    url: &UrlParts,
    request: &[u8],
    output: &str,
) -> Result<HttpResponse, String> {
    let ip = resolve_host(&url.host)?;
    println!(
        "[yt] resolved {} -> {}.{}.{}.{}",
        url.host, ip[0], ip[1], ip[2], ip[3]
    );

    let mut socket =
        Socket::new_with_domain(SocketDomain::Inet4, SocketType::Stream, SocketProtocol::Tcp)
            .map_err(|_| String::from("failed to create TCP socket"))?;
    socket
        .connect_inet(Inet4SocketAddress::new(ip, url.port))
        .map_err(|_| String::from("TCP connect failed"))?;

    let config = tls_client_config()?;
    let server_name = ServerName::try_from(url.host.clone())
        .map_err(|_| String::from("invalid TLS server name"))?;
    let mut tls = rustls::client::UnbufferedClientConnection::new(Arc::new(config), server_name)
        .map_err(|err| format!("TLS connection init failed: {:?}", err))?;

    let mut incoming_tls = Vec::new();
    let mut outgoing_tls = vec![0u8; 64 * 1024];
    let mut pending_out_len = 0usize;
    let mut request_sent = false;
    let mut header_buffer = Vec::new();
    let mut headers = None::<String>;
    let mut status = 0u16;
    let mut content_length = None::<usize>;
    let mut body_written = 0usize;
    let mut output_started = false;
    let mut error_body = Vec::new();

    loop {
        let discard;
        let mut should_read = false;
        let mut closed = false;

        {
            let tls_status = tls.process_tls_records(incoming_tls.as_mut_slice());
            discard = tls_status.discard;
            match tls_status
                .state
                .map_err(|err| format!("TLS state failed: {:?}", err))?
            {
                ConnectionState::EncodeTlsData(mut encode) => {
                    pending_out_len = encode
                        .encode(&mut outgoing_tls)
                        .map_err(|err| format!("TLS encode failed: {:?}", err))?;
                }
                ConnectionState::TransmitTlsData(transmit) => {
                    if pending_out_len > 0 {
                        write_all(
                            &mut socket,
                            &outgoing_tls[..pending_out_len],
                            "TLS handshake",
                        )?;
                        pending_out_len = 0;
                    }
                    transmit.done();
                }
                ConnectionState::BlockedHandshake => {
                    should_read = true;
                }
                ConnectionState::WriteTraffic(mut write_traffic) => {
                    if request_sent {
                        should_read = true;
                    } else {
                        let written = write_traffic
                            .encrypt(request, &mut outgoing_tls)
                            .map_err(|err| format!("TLS HTTP request encrypt failed: {:?}", err))?;
                        write_all(&mut socket, &outgoing_tls[..written], "TLS HTTP request")?;
                        request_sent = true;
                    }
                }
                ConnectionState::ReadTraffic(mut read_traffic) => {
                    while let Some(record) = read_traffic.next_record() {
                        let record = record.map_err(|err| format!("TLS read failed: {:?}", err))?;
                        if headers.is_none() {
                            header_buffer.extend_from_slice(record.payload);
                            if header_buffer.len() > MAX_HEADER_BYTES
                                && find_bytes(&header_buffer, b"\r\n\r\n").is_none()
                            {
                                return Err(String::from("HTTP headers are too large"));
                            }
                            let Some(header_end) =
                                find_bytes(&header_buffer, b"\r\n\r\n").map(|pos| pos + 4)
                            else {
                                continue;
                            };
                            let parsed_headers = core::str::from_utf8(&header_buffer[..header_end])
                                .map_err(|_| String::from("HTTP headers are not UTF-8"))?
                                .to_string();
                            status = parse_status(&parsed_headers)?;
                            let is_chunked = header_value(&parsed_headers, "transfer-encoding")
                                .map(|value| value.to_ascii_lowercase().contains("chunked"))
                                .unwrap_or(false);
                            if is_chunked && (200..300).contains(&status) {
                                return Err(String::from(
                                    "streaming chunked HTTPS body is unsupported",
                                ));
                            }
                            content_length = header_value(&parsed_headers, "content-length")
                                .and_then(|value| parse_usize(value.trim()));
                            if (200..300).contains(&status) {
                                let body = &header_buffer[header_end..];
                                if body.is_empty() {
                                    touch_truncated_file(output)?;
                                    output_started = true;
                                } else {
                                    write_stream_file_chunk(output, body, !output_started)?;
                                    output_started = true;
                                    body_written += body.len();
                                }
                                header_buffer.clear();
                            } else {
                                error_body.extend_from_slice(&header_buffer[header_end..]);
                            }
                            headers = Some(parsed_headers);
                        } else if (200..300).contains(&status) {
                            write_stream_file_chunk(output, record.payload, !output_started)?;
                            output_started = true;
                            body_written += record.payload.len();
                        } else if error_body.len() < 512 {
                            let remaining = 512 - error_body.len();
                            let copy_len = remaining.min(record.payload.len());
                            error_body.extend_from_slice(&record.payload[..copy_len]);
                        }
                    }
                }
                ConnectionState::PeerClosed | ConnectionState::Closed => {
                    closed = true;
                }
                _ => return Err(String::from("unsupported TLS state")),
            }
        }

        if discard > 0 {
            incoming_tls.drain(..discard);
        }

        if let (Some(headers), true) = (headers.as_ref(), is_redirect(status)) {
            return Ok(HttpResponse {
                status,
                headers: headers.clone(),
                body: error_body,
            });
        }
        if (200..300).contains(&status) {
            if let Some(length) = content_length {
                if body_written >= length {
                    return Ok(HttpResponse {
                        status,
                        headers: headers.unwrap_or_default(),
                        body: Vec::new(),
                    });
                }
            }
        }
        if closed {
            return Ok(HttpResponse {
                status,
                headers: headers.unwrap_or_default(),
                body: error_body,
            });
        }
        if should_read {
            read_tls_from_socket(&mut socket, &mut incoming_tls)?;
        }
    }
}

fn https_request_to_socket(
    url: &UrlParts,
    request: &[u8],
    output: &mut Socket,
) -> Result<HttpResponse, String> {
    let ip = resolve_host(&url.host)?;
    println!(
        "[yt] resolved {} -> {}.{}.{}.{}",
        url.host, ip[0], ip[1], ip[2], ip[3]
    );

    let mut tcp =
        Socket::new_with_domain(SocketDomain::Inet4, SocketType::Stream, SocketProtocol::Tcp)
            .map_err(|_| String::from("failed to create TCP socket"))?;
    tcp.connect_inet(Inet4SocketAddress::new(ip, url.port))
        .map_err(|_| String::from("TCP connect failed"))?;

    let config = tls_client_config()?;
    let server_name = ServerName::try_from(url.host.clone())
        .map_err(|_| String::from("invalid TLS server name"))?;
    let mut tls = rustls::client::UnbufferedClientConnection::new(Arc::new(config), server_name)
        .map_err(|err| format!("TLS connection init failed: {:?}", err))?;

    let mut incoming_tls = Vec::new();
    let mut outgoing_tls = vec![0u8; 64 * 1024];
    let mut pending_out_len = 0usize;
    let mut request_sent = false;
    let mut header_buffer = Vec::new();
    let mut headers = None::<String>;
    let mut status = 0u16;
    let mut content_length = None::<usize>;
    let mut body_written = 0usize;
    let mut error_body = Vec::new();

    loop {
        let discard;
        let mut should_read = false;
        let mut closed = false;

        {
            let tls_status = tls.process_tls_records(incoming_tls.as_mut_slice());
            discard = tls_status.discard;
            match tls_status
                .state
                .map_err(|err| format!("TLS state failed: {:?}", err))?
            {
                ConnectionState::EncodeTlsData(mut encode) => {
                    pending_out_len = encode
                        .encode(&mut outgoing_tls)
                        .map_err(|err| format!("TLS encode failed: {:?}", err))?;
                }
                ConnectionState::TransmitTlsData(transmit) => {
                    if pending_out_len > 0 {
                        write_all(&mut tcp, &outgoing_tls[..pending_out_len], "TLS handshake")?;
                        pending_out_len = 0;
                    }
                    transmit.done();
                }
                ConnectionState::BlockedHandshake => {
                    should_read = true;
                }
                ConnectionState::WriteTraffic(mut write_traffic) => {
                    if request_sent {
                        should_read = true;
                    } else {
                        let written = write_traffic
                            .encrypt(request, &mut outgoing_tls)
                            .map_err(|err| format!("TLS HTTP request encrypt failed: {:?}", err))?;
                        write_all(&mut tcp, &outgoing_tls[..written], "TLS HTTP request")?;
                        request_sent = true;
                    }
                }
                ConnectionState::ReadTraffic(mut read_traffic) => {
                    while let Some(record) = read_traffic.next_record() {
                        let record = record.map_err(|err| format!("TLS read failed: {:?}", err))?;
                        if headers.is_none() {
                            header_buffer.extend_from_slice(record.payload);
                            if header_buffer.len() > MAX_HEADER_BYTES
                                && find_bytes(&header_buffer, b"\r\n\r\n").is_none()
                            {
                                return Err(String::from("HTTP headers are too large"));
                            }
                            let Some(header_end) =
                                find_bytes(&header_buffer, b"\r\n\r\n").map(|pos| pos + 4)
                            else {
                                continue;
                            };
                            let parsed_headers = core::str::from_utf8(&header_buffer[..header_end])
                                .map_err(|_| String::from("HTTP headers are not UTF-8"))?
                                .to_string();
                            status = parse_status(&parsed_headers)?;
                            let is_chunked = header_value(&parsed_headers, "transfer-encoding")
                                .map(|value| value.to_ascii_lowercase().contains("chunked"))
                                .unwrap_or(false);
                            if is_chunked && (200..300).contains(&status) {
                                return Err(String::from(
                                    "streaming chunked HTTPS body is unsupported",
                                ));
                            }
                            content_length = header_value(&parsed_headers, "content-length")
                                .and_then(|value| parse_usize(value.trim()));
                            if (200..300).contains(&status) {
                                let body = &header_buffer[header_end..];
                                if !body.is_empty() {
                                    write_all(output, body, "media stream")?;
                                    body_written += body.len();
                                }
                                header_buffer.clear();
                            } else {
                                error_body.extend_from_slice(&header_buffer[header_end..]);
                            }
                            headers = Some(parsed_headers);
                        } else if (200..300).contains(&status) {
                            write_all(output, record.payload, "media stream")?;
                            body_written += record.payload.len();
                        } else if error_body.len() < 512 {
                            let remaining = 512 - error_body.len();
                            let copy_len = remaining.min(record.payload.len());
                            error_body.extend_from_slice(&record.payload[..copy_len]);
                        }
                    }
                }
                ConnectionState::PeerClosed | ConnectionState::Closed => {
                    closed = true;
                }
                _ => return Err(String::from("unsupported TLS state")),
            }
        }

        if discard > 0 {
            incoming_tls.drain(..discard);
        }

        if let (Some(headers), true) = (headers.as_ref(), is_redirect(status)) {
            return Ok(HttpResponse {
                status,
                headers: headers.clone(),
                body: error_body,
            });
        }
        if (200..300).contains(&status) {
            if let Some(length) = content_length {
                if body_written >= length {
                    return Ok(HttpResponse {
                        status,
                        headers: headers.unwrap_or_default(),
                        body: Vec::new(),
                    });
                }
            }
        }
        if closed {
            return Ok(HttpResponse {
                status,
                headers: headers.unwrap_or_default(),
                body: error_body,
            });
        }
        if should_read {
            read_tls_from_socket(&mut tcp, &mut incoming_tls)?;
        }
    }
}

fn https_request(url: &UrlParts, request: &[u8]) -> Result<HttpResponse, String> {
    let ip = resolve_host(&url.host)?;
    println!(
        "[yt] resolved {} -> {}.{}.{}.{}",
        url.host, ip[0], ip[1], ip[2], ip[3]
    );

    let mut socket =
        Socket::new_with_domain(SocketDomain::Inet4, SocketType::Stream, SocketProtocol::Tcp)
            .map_err(|_| String::from("failed to create TCP socket"))?;
    socket
        .connect_inet(Inet4SocketAddress::new(ip, url.port))
        .map_err(|_| String::from("TCP connect failed"))?;

    let config = tls_client_config()?;
    let server_name = ServerName::try_from(url.host.clone())
        .map_err(|_| String::from("invalid TLS server name"))?;
    let mut tls = rustls::client::UnbufferedClientConnection::new(Arc::new(config), server_name)
        .map_err(|err| format!("TLS connection init failed: {:?}", err))?;

    let mut incoming_tls = Vec::new();
    let mut outgoing_tls = vec![0u8; 64 * 1024];
    let mut pending_out_len = 0usize;
    let mut request_sent = false;
    let mut plaintext = Vec::new();

    loop {
        let mut discard;
        let mut should_read = false;
        let mut closed = false;

        {
            let status = tls.process_tls_records(incoming_tls.as_mut_slice());
            discard = status.discard;
            match status
                .state
                .map_err(|err| format!("TLS state failed: {:?}", err))?
            {
                ConnectionState::EncodeTlsData(mut encode) => {
                    pending_out_len = encode
                        .encode(&mut outgoing_tls)
                        .map_err(|err| format!("TLS encode failed: {:?}", err))?;
                    if LOG_TLS_IO {
                        println!("[yt] TLS encode {} bytes", pending_out_len);
                    }
                }
                ConnectionState::TransmitTlsData(transmit) => {
                    if pending_out_len > 0 {
                        if LOG_TLS_IO {
                            println!("[yt] TLS transmit {} bytes", pending_out_len);
                        }
                        write_all(
                            &mut socket,
                            &outgoing_tls[..pending_out_len],
                            "TLS handshake",
                        )?;
                        pending_out_len = 0;
                    }
                    transmit.done();
                }
                ConnectionState::BlockedHandshake => {
                    if LOG_TLS_IO {
                        println!("[yt] TLS blocked handshake; reading");
                    }
                    should_read = true;
                }
                ConnectionState::WriteTraffic(mut write_traffic) => {
                    if request_sent {
                        should_read = true;
                    } else {
                        let written = write_traffic
                            .encrypt(request, &mut outgoing_tls)
                            .map_err(|err| format!("TLS HTTP request encrypt failed: {:?}", err))?;
                        if LOG_TLS_IO {
                            println!("[yt] TLS HTTP request {} bytes", written);
                        }
                        write_all(&mut socket, &outgoing_tls[..written], "TLS HTTP request")?;
                        request_sent = true;
                    }
                }
                ConnectionState::ReadTraffic(mut read_traffic) => {
                    while let Some(record) = read_traffic.next_record() {
                        let record = record.map_err(|err| format!("TLS read failed: {:?}", err))?;
                        if plaintext.len() + record.payload.len() > MAX_HTTPS_RESPONSE_BYTES {
                            return Err(String::from("HTTPS response is too large"));
                        }
                        if LOG_TLS_IO {
                            println!("[yt] TLS plaintext {} bytes", record.payload.len());
                        }
                        plaintext.extend_from_slice(record.payload);
                        discard = discard.saturating_add(record.discard);
                    }
                }
                ConnectionState::PeerClosed | ConnectionState::Closed => {
                    if LOG_TLS_IO {
                        println!("[yt] TLS closed");
                    }
                    closed = true;
                }
                _ => {
                    return Err(String::from("unsupported TLS state"));
                }
            }
        }

        if discard > 0 {
            incoming_tls.drain(..discard);
        }

        if closed {
            break;
        }
        if request_sent && http_response_complete(&plaintext) {
            return parse_complete_http_response(plaintext);
        }
        if should_read {
            read_tls_from_socket(&mut socket, &mut incoming_tls)?;
        }
    }

    parse_complete_http_response(plaintext)
}

fn http_response_complete(received: &[u8]) -> bool {
    let header_end = match find_bytes(received, b"\r\n\r\n") {
        Some(pos) => pos + 4,
        None => return false,
    };
    let headers = match core::str::from_utf8(&received[..header_end]) {
        Ok(headers) => headers,
        Err(_) => return false,
    };
    let body = &received[header_end..];

    if header_value(headers, "transfer-encoding")
        .map(|value| value.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
    {
        return chunked_body_complete(body);
    }

    if let Some(length) = header_value(headers, "content-length") {
        if let Some(content_length) = parse_usize(length.trim()) {
            return body.len() >= content_length;
        }
    }

    false
}

fn chunked_body_complete(buffer: &[u8]) -> bool {
    let mut offset = 0usize;

    loop {
        let line_end = match find_bytes_from(buffer, b"\r\n", offset) {
            Some(pos) => pos,
            None => return false,
        };
        let line = match core::str::from_utf8(&buffer[offset..line_end]) {
            Ok(line) => line,
            Err(_) => return false,
        };
        let size = match parse_hex_usize(line.split(';').next().unwrap_or("").trim()) {
            Some(size) => size,
            None => return false,
        };
        offset = line_end + 2;
        if size == 0 {
            return buffer.len() >= offset + 2;
        }
        if buffer.len() < offset + size + 2 {
            return false;
        }
        offset += size + 2;
    }
}

fn read_tls_from_socket(socket: &mut Socket, buffer: &mut Vec<u8>) -> Result<(), String> {
    wait_readable(socket, TLS_TIMEOUT_NS)?;
    let mut chunk = [0u8; 8192];
    let n = socket
        .read(&mut chunk)
        .map_err(|_| String::from("TLS socket read failed"))?;
    if n == 0 {
        return Err(String::from("TLS connection closed unexpectedly"));
    }
    if LOG_TLS_IO {
        println!("[yt] TLS socket read {} bytes", n);
    }
    buffer.extend_from_slice(&chunk[..n]);
    Ok(())
}

fn parse_complete_http_response(received: Vec<u8>) -> Result<HttpResponse, String> {
    let header_end = find_bytes(&received, b"\r\n\r\n")
        .map(|pos| pos + 4)
        .ok_or_else(|| String::from("HTTP headers not found"))?;
    let headers = core::str::from_utf8(&received[..header_end])
        .map_err(|_| String::from("HTTP headers are not UTF-8"))?
        .to_string();
    let status = parse_status(&headers)?;
    let mut body = received[header_end..].to_vec();

    if header_value(&headers, "transfer-encoding")
        .map(|value| value.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
    {
        body = decode_chunked_buffer(&body)?;
    } else if let Some(length) = header_value(&headers, "content-length") {
        let content_length = parse_usize(length.trim())
            .ok_or_else(|| String::from("invalid Content-Length header"))?;
        if body.len() > content_length {
            body.truncate(content_length);
        }
    }

    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked_buffer(buffer: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoded = Vec::new();
    let mut offset = 0usize;

    loop {
        let line_end = find_bytes_from(buffer, b"\r\n", offset)
            .ok_or_else(|| String::from("invalid chunked body"))?;
        let line = core::str::from_utf8(&buffer[offset..line_end])
            .map_err(|_| String::from("invalid chunk size"))?;
        let size = parse_hex_usize(line.split(';').next().unwrap_or("").trim())
            .ok_or_else(|| String::from("invalid chunk size"))?;
        offset = line_end + 2;
        if size == 0 {
            break;
        }
        if buffer.len() < offset + size + 2 {
            return Err(String::from("truncated chunked body"));
        }
        decoded.extend_from_slice(&buffer[offset..offset + size]);
        offset += size + 2;
    }

    Ok(decoded)
}

fn read_chunked_body(socket: &mut Socket, buffer: &mut Vec<u8>) -> Result<(), String> {
    let mut decoded = Vec::new();
    let mut offset = 0usize;

    loop {
        let line_end = loop {
            if let Some(pos) = find_bytes_from(buffer, b"\r\n", offset) {
                break pos;
            }
            read_more(socket, buffer)?;
        };
        let line = core::str::from_utf8(&buffer[offset..line_end])
            .map_err(|_| String::from("invalid chunk size"))?;
        let size = parse_hex_usize(line.split(';').next().unwrap_or("").trim())
            .ok_or_else(|| String::from("invalid chunk size"))?;
        offset = line_end + 2;
        if size == 0 {
            break;
        }
        while buffer.len() < offset + size + 2 {
            read_more(socket, buffer)?;
        }
        decoded.extend_from_slice(&buffer[offset..offset + size]);
        offset += size + 2;
    }

    *buffer = decoded;
    Ok(())
}

fn read_more(socket: &mut Socket, buffer: &mut Vec<u8>) -> Result<(), String> {
    wait_readable(socket, HTTP_TIMEOUT_NS)?;
    let mut chunk = [0u8; 8192];
    let n = socket
        .read(&mut chunk)
        .map_err(|_| String::from("HTTP body read failed"))?;
    if n == 0 {
        return Err(String::from("unexpected EOF"));
    }
    buffer.extend_from_slice(&chunk[..n]);
    Ok(())
}

fn write_body(path: &str, body: Vec<u8>) -> Result<(), String> {
    let mut options = OpenOptions::new();
    let mut file = options
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|_| format!("failed to open {}", path))?;
    write_all(&mut file, &body, "output file")?;
    Ok(())
}

fn touch_truncated_file(path: &str) -> Result<(), String> {
    let mut options = OpenOptions::new();
    let _file = options
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|_| format!("failed to open {}", path))?;
    Ok(())
}

fn write_stream_file_chunk(path: &str, bytes: &[u8], truncate: bool) -> Result<(), String> {
    let mut options = OpenOptions::new();
    let mut file = if truncate {
        options.write(true).create(true).truncate(true).open(path)
    } else {
        options.append(true).create(true).open(path)
    }
    .map_err(|_| format!("failed to open {}", path))?;
    write_all(&mut file, bytes, "output file")
}

fn print_http_error_body(status: u16, body: &[u8]) {
    if body.is_empty() {
        return;
    }
    let max_len = body.len().min(512);
    match core::str::from_utf8(&body[..max_len]) {
        Ok(text) => println!("[yt] HTTP {} body: {}", status, text),
        Err(_) => println!("[yt] HTTP {} body: {} bytes", status, body.len()),
    }
}

fn derive_audio_output_path(video_output: &str) -> String {
    if let Some(dot) = video_output.rfind('.') {
        return format!("{}.m4a", &video_output[..dot]);
    }
    format!("{}.m4a", video_output)
}

fn display_path(path: &str) -> String {
    const MAX_LEN: usize = 160;
    if path.len() <= MAX_LEN {
        return path.to_string();
    }
    format!("{}...", &path[..MAX_LEN])
}

#[derive(Debug)]
struct ScarletTimeProvider;

impl TimeProvider for ScarletTimeProvider {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(
            1_780_000_000,
        )))
    }
}

#[derive(Debug)]
struct InsecureServerVerifier;

impl ServerCertVerifier for InsecureServerVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

fn tls_client_config() -> Result<ClientConfig, String> {
    let mut config = ClientConfig::builder_with_details(
        Arc::new(rustls::crypto::ring::default_provider()),
        Arc::new(ScarletTimeProvider),
    )
    .with_safe_default_protocol_versions()
    .map_err(|err| format!("TLS protocol version setup failed: {:?}", err))?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(InsecureServerVerifier))
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

fn resolve_youtube_media(input: &str) -> Result<MediaSelection, String> {
    let video_id =
        youtube_video_id(input).ok_or_else(|| String::from("YouTube video id not found"))?;
    let watch_url = format!(
        "https://www.youtube.com/watch?v={}&bpctr=9999999999&has_verified=1",
        video_id
    );
    println!("[yt] loading YouTube watch page");
    let response = http_get(
        &parse_url(&watch_url)?,
        DEFAULT_USER_AGENT,
        DEFAULT_EXTRA_HEADERS,
    )?;
    if response.status < 200 || response.status >= 300 {
        return Err(format!(
            "YouTube watch page HTTP status {}",
            response.status
        ));
    }
    let page = core::str::from_utf8(&response.body)
        .map_err(|_| String::from("YouTube watch page is not UTF-8"))?;
    if let Some(media) = select_youtube_dash_mp4(page) {
        println!(
            "[yt] selected DASH MP4 streams from watch page: {}p + AAC",
            media.video_height
        );
        return Ok(media.into_selection(DEFAULT_USER_AGENT, YOUTUBE_MEDIA_EXTRA_HEADERS));
    }

    println!("[yt] no DASH MP4 pair in watch page; trying YouTube player API");
    let api_key =
        youtube_innertube_api_key(page).unwrap_or_else(fallback_youtube_innertube_api_key);
    let client_version = youtube_innertube_client_version(page)
        .unwrap_or_else(|| YOUTUBE_WEB_CLIENT_VERSION.to_string());
    let visitor_data = youtube_visitor_data(page);
    match resolve_youtube_media_url_via_player_api(
        &video_id,
        &api_key,
        &client_version,
        visitor_data.as_deref(),
    ) {
        Ok(media) => Ok(media),
        Err(error) => {
            println!("[yt] YouTube player API fallback failed: {}", error);
            if let Some(url) = select_youtube_progressive_mp4(page) {
                println!("[yt] selected progressive MP4 stream from watch page");
                Ok(MediaSelection {
                    video_url: url,
                    audio_url: None,
                    user_agent: DEFAULT_USER_AGENT,
                    extra_headers: YOUTUBE_MEDIA_EXTRA_HEADERS,
                })
            } else {
                Err(error)
            }
        }
    }
}

fn resolve_youtube_media_url_via_player_api(
    video_id: &str,
    api_key: &str,
    client_version: &str,
    visitor_data: Option<&str>,
) -> Result<MediaSelection, String> {
    let clients = [
        YoutubeClientSpec::android_vr(),
        YoutubeClientSpec::web(client_version),
        YoutubeClientSpec::mweb(),
        YoutubeClientSpec::android(),
        YoutubeClientSpec::ios(),
    ];
    let mut last_error = String::from("no YouTube player clients tried");
    let mut progressive_fallback = None;

    for client in clients {
        match try_youtube_player_client(video_id, api_key, &client, visitor_data) {
            Ok(YoutubeClientMedia::Dash(media)) => {
                return Ok(media.into_selection(client.user_agent, YOUTUBE_MEDIA_EXTRA_HEADERS));
            }
            Ok(YoutubeClientMedia::Progressive(url)) => {
                if progressive_fallback.is_none() {
                    progressive_fallback = Some(MediaSelection {
                        video_url: url,
                        audio_url: None,
                        user_agent: client.user_agent,
                        extra_headers: YOUTUBE_MEDIA_EXTRA_HEADERS,
                    });
                }
                last_error = format!("{} returned only progressive MP4", client.label);
            }
            Ok(YoutubeClientMedia::None) => {
                last_error = format!("{} returned no direct MP4 streams", client.label);
            }
            Err(error) => {
                println!("[yt] YouTube {} player API failed: {}", client.label, error);
                last_error = error;
            }
        }
    }

    progressive_fallback.ok_or(last_error)
}

fn try_youtube_player_client(
    video_id: &str,
    api_key: &str,
    client: &YoutubeClientSpec<'_>,
    visitor_data: Option<&str>,
) -> Result<YoutubeClientMedia, String> {
    let api_url = format!(
        "https://www.youtube.com/youtubei/v1/player?key={}&prettyPrint=false",
        api_key
    );
    let body = youtube_player_request_body(video_id, client, visitor_data);
    let mut extra_headers = format!(
        "Origin: https://www.youtube.com\r\nReferer: https://www.youtube.com/watch?v={}\r\nX-YouTube-Client-Name: {}\r\nX-YouTube-Client-Version: {}\r\n",
        video_id, client.client_id, client.client_version
    );
    if let Some(visitor_data) = visitor_data {
        extra_headers.push_str("X-Goog-Visitor-Id: ");
        extra_headers.push_str(visitor_data);
        extra_headers.push_str("\r\n");
    }

    println!("[yt] loading YouTube {} player API", client.label);
    let response = https_post_json(
        &parse_url(&api_url)?,
        &body,
        client.user_agent,
        &extra_headers,
    )?;
    if response.status < 200 || response.status >= 300 {
        if let Ok(body) = core::str::from_utf8(&response.body) {
            println!(
                "[yt] YouTube {} player API error body: {}",
                client.label, body
            );
        }
        return Err(format!(
            "YouTube {} player API HTTP status {}",
            client.label, response.status
        ));
    }
    let payload = core::str::from_utf8(&response.body)
        .map_err(|_| String::from("YouTube player API response is not UTF-8"))?;
    let stats = youtube_format_stats(payload);
    println!(
        "[yt] {} formats: total={} progressive_mp4={} h264_video={} aac_audio={} direct={} sig={} cipher_s={} dash_direct={} dash_sig={} dash_s={}",
        client.label,
        stats.total,
        stats.progressive_mp4,
        stats.h264_video_only,
        stats.aac_audio_only,
        stats.direct_url,
        stats.signature_url,
        stats.cipher_signature,
        stats.dash_direct,
        stats.dash_sig,
        stats.dash_cipher_s
    );
    if let Some(reason) = youtube_playability_reason(payload) {
        println!("[yt] {} playability: {}", client.label, reason);
    }
    if let Some(media) = select_youtube_dash_mp4(payload) {
        println!(
            "[yt] selected DASH MP4 streams from {} player API: {}p + AAC",
            client.label, media.video_height
        );
        return Ok(YoutubeClientMedia::Dash(media));
    }
    if let Some(url) = select_youtube_progressive_mp4(payload) {
        println!(
            "[yt] found progressive MP4 stream from {} player API",
            client.label
        );
        return Ok(YoutubeClientMedia::Progressive(url));
    }
    Ok(YoutubeClientMedia::None)
}

enum YoutubeClientMedia {
    Dash(YoutubeDashSelection),
    Progressive(String),
    None,
}

struct YoutubeClientSpec<'a> {
    label: &'static str,
    client_name: &'static str,
    client_version: &'a str,
    client_id: u32,
    user_agent: &'static str,
    platform: YoutubeClientPlatform,
}

impl<'a> YoutubeClientSpec<'a> {
    fn web(client_version: &'a str) -> Self {
        Self {
            label: "web",
            client_name: "WEB",
            client_version,
            client_id: 1,
            user_agent: "Mozilla/5.0 (X11; Scarlet) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0 Safari/537.36",
            platform: YoutubeClientPlatform::Web,
        }
    }

    fn mweb() -> Self {
        Self {
            label: "mweb",
            client_name: "MWEB",
            client_version: YOUTUBE_MWEB_CLIENT_VERSION,
            client_id: 2,
            user_agent: "Mozilla/5.0 (iPad; CPU OS 16_7_10 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1,gzip(gfe)",
            platform: YoutubeClientPlatform::Web,
        }
    }

    fn android() -> Self {
        Self {
            label: "android",
            client_name: "ANDROID",
            client_version: YOUTUBE_ANDROID_CLIENT_VERSION,
            client_id: 3,
            user_agent: "com.google.android.youtube/21.02.35 (Linux; U; Android 11) gzip",
            platform: YoutubeClientPlatform::Android,
        }
    }

    fn android_vr() -> Self {
        Self {
            label: "android_vr",
            client_name: "ANDROID_VR",
            client_version: YOUTUBE_ANDROID_VR_CLIENT_VERSION,
            client_id: 28,
            user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
            platform: YoutubeClientPlatform::AndroidVr,
        }
    }

    fn ios() -> Self {
        Self {
            label: "ios",
            client_name: "IOS",
            client_version: YOUTUBE_IOS_CLIENT_VERSION,
            client_id: 5,
            user_agent: "com.google.ios.youtube/21.02.3 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
            platform: YoutubeClientPlatform::Ios,
        }
    }
}

enum YoutubeClientPlatform {
    Web,
    Android,
    AndroidVr,
    Ios,
}

fn youtube_player_request_body(
    video_id: &str,
    client: &YoutubeClientSpec<'_>,
    visitor_data: Option<&str>,
) -> String {
    let mut body = format!(
        concat!(
            "{{",
            "\"context\":{{",
            "\"client\":{{",
            "\"clientName\":\"{}\",",
            "\"clientVersion\":\"{}\",",
            "\"hl\":\"en\",",
            "\"gl\":\"US\",",
            "\"utcOffsetMinutes\":0"
        ),
        client.client_name, client.client_version
    );

    match client.platform {
        YoutubeClientPlatform::Web => {}
        YoutubeClientPlatform::Android => {
            body.push_str(
                ",\"androidSdkVersion\":30,\"userAgent\":\"com.google.android.youtube/21.02.35 (Linux; U; Android 11) gzip\",\"osName\":\"Android\",\"osVersion\":\"11\"",
            );
        }
        YoutubeClientPlatform::AndroidVr => {
            body.push_str(
                ",\"deviceMake\":\"Oculus\",\"deviceModel\":\"Quest 3\",\"androidSdkVersion\":32,\"userAgent\":\"com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip\",\"osName\":\"Android\",\"osVersion\":\"12L\"",
            );
        }
        YoutubeClientPlatform::Ios => {
            body.push_str(
                ",\"deviceMake\":\"Apple\",\"deviceModel\":\"iPhone16,2\",\"userAgent\":\"com.google.ios.youtube/21.02.3 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)\",\"osName\":\"iPhone\",\"osVersion\":\"18.3.2.22D82\"",
            );
        }
    }

    if let Some(visitor_data) = visitor_data {
        body.push_str(",\"visitorData\":\"");
        push_json_escaped(&mut body, visitor_data);
        body.push('"');
    }

    body.push_str("}},");
    body.push_str("\"videoId\":\"");
    push_json_escaped(&mut body, video_id);
    body.push_str("\",\"playbackContext\":{\"contentPlaybackContext\":{\"html5Preference\":\"HTML5_PREF_WANTS\"}},\"contentCheckOk\":true,\"racyCheckOk\":true}");
    body
}

struct YoutubeFormatStats {
    total: usize,
    progressive_mp4: usize,
    h264_video_only: usize,
    aac_audio_only: usize,
    direct_url: usize,
    signature_url: usize,
    cipher_signature: usize,
    dash_direct: usize,
    dash_sig: usize,
    dash_cipher_s: usize,
}

fn youtube_format_stats(payload: &str) -> YoutubeFormatStats {
    let mut stats = YoutubeFormatStats {
        total: 0,
        progressive_mp4: 0,
        h264_video_only: 0,
        aac_audio_only: 0,
        direct_url: 0,
        signature_url: 0,
        cipher_signature: 0,
        dash_direct: 0,
        dash_sig: 0,
        dash_cipher_s: 0,
    };

    if let Some(formats) = youtube_formats_payload(payload) {
        for object in JsonObjects::new(formats) {
            collect_youtube_format_stats(object, &mut stats);
        }
    }

    if let Some(adaptive_formats) = youtube_adaptive_formats_payload(payload) {
        for object in JsonObjects::new(adaptive_formats) {
            collect_youtube_format_stats(object, &mut stats);
        }
    }

    stats
}

fn collect_youtube_format_stats(object: &str, stats: &mut YoutubeFormatStats) {
    stats.total += 1;
    let mime = json_string_field(object, "mimeType").unwrap_or_default();
    let is_progressive_mp4 = mime.contains("video/mp4") && mime.contains("mp4a");
    if mime.contains("video/mp4") && mime.contains("avc1") && !mime.contains("mp4a") {
        stats.h264_video_only += 1;
        collect_youtube_dash_url_stats(object, stats);
    }
    if mime.contains("audio/mp4") && mime.contains("mp4a") {
        stats.aac_audio_only += 1;
        collect_youtube_dash_url_stats(object, stats);
    }
    if !is_progressive_mp4 {
        return;
    }
    stats.progressive_mp4 += 1;
    if json_string_field(object, "url").is_some() {
        stats.direct_url += 1;
        return;
    }
    let cipher = json_string_field(object, "signatureCipher")
        .or_else(|| json_string_field(object, "cipher"));
    if let Some(cipher) = cipher {
        if form_value(&cipher, "sig")
            .or_else(|| form_value(&cipher, "signature"))
            .is_some()
        {
            stats.signature_url += 1;
        } else if form_value(&cipher, "s").is_some() {
            stats.cipher_signature += 1;
        }
    }
}

fn collect_youtube_dash_url_stats(object: &str, stats: &mut YoutubeFormatStats) {
    if json_string_field(object, "url").is_some() {
        stats.dash_direct += 1;
        return;
    }
    let cipher = json_string_field(object, "signatureCipher")
        .or_else(|| json_string_field(object, "cipher"));
    if let Some(cipher) = cipher {
        if form_value(&cipher, "sig")
            .or_else(|| form_value(&cipher, "signature"))
            .is_some()
        {
            stats.dash_sig += 1;
        } else if form_value(&cipher, "s").is_some() {
            stats.dash_cipher_s += 1;
        }
    }
}

struct YoutubeDashSelection {
    video_url: String,
    audio_url: String,
    video_height: usize,
}

impl YoutubeDashSelection {
    fn into_selection(
        self,
        user_agent: &'static str,
        extra_headers: &'static str,
    ) -> MediaSelection {
        MediaSelection {
            video_url: self.video_url,
            audio_url: Some(self.audio_url),
            user_agent,
            extra_headers,
        }
    }
}

fn select_youtube_dash_mp4(page: &str) -> Option<YoutubeDashSelection> {
    let formats = youtube_adaptive_formats_payload(page)?;
    let mut best_video_url = None;
    let mut best_video_height = 0usize;
    let mut best_audio_url = None;
    let mut best_audio_bitrate = 0usize;

    for object in JsonObjects::new(formats) {
        let Some(mime) = json_string_field(object, "mimeType") else {
            continue;
        };
        if mime.contains("video/mp4") && mime.contains("avc1") && !mime.contains("mp4a") {
            let Some(url) = youtube_format_url(object) else {
                continue;
            };
            let height = json_usize_field(object, "height").unwrap_or(0);
            if best_video_url.is_none() || height > best_video_height {
                best_video_height = height;
                best_video_url = Some(url);
            }
        } else if mime.contains("audio/mp4") && mime.contains("mp4a") {
            let Some(url) = youtube_format_url(object) else {
                continue;
            };
            let bitrate = json_usize_field(object, "averageBitrate")
                .or_else(|| json_usize_field(object, "bitrate"))
                .unwrap_or(0);
            if best_audio_url.is_none() || bitrate > best_audio_bitrate {
                best_audio_bitrate = bitrate;
                best_audio_url = Some(url);
            }
        }
    }

    Some(YoutubeDashSelection {
        video_url: best_video_url?,
        audio_url: best_audio_url?,
        video_height: best_video_height,
    })
}

fn youtube_playability_reason(payload: &str) -> Option<String> {
    json_string_field(payload, "reason").or_else(|| json_string_field(payload, "status"))
}

fn youtube_innertube_api_key(page: &str) -> Option<String> {
    json_string_field(page, "INNERTUBE_API_KEY")
}

fn youtube_innertube_client_version(page: &str) -> Option<String> {
    json_string_field(page, "INNERTUBE_CLIENT_VERSION")
}

fn youtube_visitor_data(page: &str) -> Option<String> {
    json_string_field(page, "VISITOR_DATA").or_else(|| json_string_field(page, "visitorData"))
}

fn select_youtube_progressive_mp4(page: &str) -> Option<String> {
    let formats = youtube_formats_payload(page)?;
    let mut best_url = None;
    let mut best_height = 0usize;

    for object in JsonObjects::new(formats) {
        let Some(mime) = json_string_field(object, "mimeType") else {
            continue;
        };
        if !mime.contains("video/mp4") || !mime.contains("mp4a") {
            continue;
        }
        let Some(url) = youtube_format_url(object) else {
            continue;
        };
        let height = json_usize_field(object, "height").unwrap_or(0);
        if best_url.is_none() || height > best_height {
            best_height = height;
            best_url = Some(url);
        }
    }

    best_url
}

fn youtube_formats_payload(page: &str) -> Option<&str> {
    let formats_key = "\"formats\":[";
    let formats_start = page.find(formats_key)? + formats_key.len() - 1;
    let formats_end = find_matching_json(page, formats_start, b'[', b']')?;
    Some(&page[formats_start + 1..formats_end])
}

fn youtube_adaptive_formats_payload(page: &str) -> Option<&str> {
    let formats_key = "\"adaptiveFormats\":[";
    let formats_start = page.find(formats_key)? + formats_key.len() - 1;
    let formats_end = find_matching_json(page, formats_start, b'[', b']')?;
    Some(&page[formats_start + 1..formats_end])
}

fn youtube_format_url(object: &str) -> Option<String> {
    if let Some(url) = json_string_field(object, "url") {
        return Some(url);
    }

    let cipher = json_string_field(object, "signatureCipher")
        .or_else(|| json_string_field(object, "cipher"))?;
    let mut url = form_value(&cipher, "url")?;
    if let Some(sig) = form_value(&cipher, "sig").or_else(|| form_value(&cipher, "signature")) {
        let sig_name = form_value(&cipher, "sp").unwrap_or_else(|| String::from("signature"));
        append_query_param(&mut url, &sig_name, &sig);
        return Some(url);
    }

    if form_value(&cipher, "s").is_some() {
        return None;
    }

    Some(url)
}

struct JsonObjects<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> JsonObjects<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, offset: 0 }
    }
}

impl<'a> Iterator for JsonObjects<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.input.as_bytes();
        while self.offset < bytes.len() && bytes[self.offset] != b'{' {
            self.offset += 1;
        }
        if self.offset >= bytes.len() {
            return None;
        }
        let start = self.offset;
        let end = find_matching_json(self.input, start, b'{', b'}')?;
        self.offset = end + 1;
        Some(&self.input[start..=end])
    }
}

fn find_matching_json(input: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = input.as_bytes();
    if *bytes.get(start)? != open {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }

        if *byte == b'"' {
            in_string = true;
        } else if *byte == open {
            depth += 1;
        } else if *byte == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
}

fn json_string_field(input: &str, name: &str) -> Option<String> {
    let pattern = format!("\"{}\":\"", name);
    let start = input.find(&pattern)? + pattern.len();
    parse_json_string_value(input, start)
}

fn push_json_escaped(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            ch if ch < ' ' => {
                out.push_str("\\u00");
                let byte = ch as u8;
                out.push(json_hex_digit(byte >> 4));
                out.push(json_hex_digit(byte & 0x0f));
            }
            ch => out.push(ch),
        }
    }
}

fn json_hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + value - 10) as char,
        _ => '0',
    }
}

fn parse_json_string_value(input: &str, start: usize) -> Option<String> {
    let mut out = String::new();
    let mut index = start;

    while index < input.len() {
        let ch = input[index..].chars().next()?;
        if ch == '"' {
            return Some(out);
        }
        if ch != '\\' {
            out.push(ch);
            index += ch.len_utf8();
            continue;
        }

        index += 1;
        let escaped = input.as_bytes().get(index).copied()?;
        match escaped {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{0008}'),
            b'f' => out.push('\u{000c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let code = parse_hex_u16(input.as_bytes().get(index + 1..index + 5)?)?;
                index += 4;
                if (0xd800..=0xdbff).contains(&code) {
                    let tail = input.as_bytes().get(index + 1..index + 7)?;
                    if tail.first().copied() != Some(b'\\') || tail.get(1).copied() != Some(b'u') {
                        return None;
                    }
                    let low = parse_hex_u16(tail.get(2..6)?)?;
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return None;
                    }
                    let high_ten = u32::from(code - 0xd800);
                    let low_ten = u32::from(low - 0xdc00);
                    let scalar = 0x10000 + ((high_ten << 10) | low_ten);
                    out.push(core::char::from_u32(scalar)?);
                    index += 6;
                } else {
                    out.push(core::char::from_u32(code as u32)?);
                }
            }
            _ => return None,
        }
        index += 1;
    }

    None
}

fn json_usize_field(input: &str, name: &str) -> Option<usize> {
    let pattern = format!("\"{}\":", name);
    let mut index = input.find(&pattern)? + pattern.len();
    let bytes = input.as_bytes();
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    let start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    parse_usize(&input[start..index])
}

fn parse_hex_u16(input: &[u8]) -> Option<u16> {
    if input.len() != 4 {
        return None;
    }
    let mut value = 0u16;
    for byte in input {
        let digit = match *byte {
            b'0'..=b'9' => *byte - b'0',
            b'a'..=b'f' => *byte - b'a' + 10,
            b'A'..=b'F' => *byte - b'A' + 10,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(digit as u16)?;
    }
    Some(value)
}

fn resolve_host(host: &str) -> Result<[u8; 4], String> {
    if let Some(ip) = parse_ipv4(host) {
        return Ok(ip);
    }

    let dns = configured_dns_server().unwrap_or(DEFAULT_DNS);
    println!(
        "[yt] DNS A {} via {}.{}.{}.{}",
        host, dns[0], dns[1], dns[2], dns[3]
    );
    let query = build_dns_query(host)?;
    let mut socket = Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Udp,
    )
    .map_err(|_| String::from("failed to create UDP socket"))?;
    socket
        .connect_inet(Inet4SocketAddress::new(dns, DNS_PORT))
        .map_err(|_| String::from("DNS UDP connect failed"))?;
    write_all(&mut socket, &query, "DNS query")?;
    wait_readable(&socket, DNS_TIMEOUT_NS)?;

    let mut response = [0u8; 1500];
    let n = socket
        .read(&mut response)
        .map_err(|_| String::from("DNS read failed"))?;
    parse_dns_a_response(&response[..n]).ok_or_else(|| String::from("DNS A record not found"))
}

fn configured_dns_server() -> Option<[u8; 4]> {
    let (status, _) = list_interfaces().ok()?;
    if status.dns_set == 1 {
        Some(status.dns_server)
    } else {
        None
    }
}

fn build_dns_query(host: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(&0x5954u16.to_be_bytes());
    out.extend_from_slice(&0x0100u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());

    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(String::from("invalid DNS name"));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    Ok(out)
}

fn parse_dns_a_response(packet: &[u8]) -> Option<[u8; 4]> {
    if packet.len() < 12 {
        return None;
    }
    if read_u16(packet, 0)? != 0x5954 {
        return None;
    }
    let qdcount = read_u16(packet, 4)? as usize;
    let ancount = read_u16(packet, 6)? as usize;
    let mut offset = 12usize;
    for _ in 0..qdcount {
        skip_dns_name(packet, &mut offset)?;
        offset = offset.checked_add(4)?;
        if offset > packet.len() {
            return None;
        }
    }
    for _ in 0..ancount {
        skip_dns_name(packet, &mut offset)?;
        let rr_type = read_u16(packet, offset)?;
        let rr_class = read_u16(packet, offset + 2)?;
        let rdlen = read_u16(packet, offset + 8)? as usize;
        offset += 10;
        if offset + rdlen > packet.len() {
            return None;
        }
        if rr_type == 1 && rr_class == 1 && rdlen == 4 {
            return Some([
                packet[offset],
                packet[offset + 1],
                packet[offset + 2],
                packet[offset + 3],
            ]);
        }
        offset += rdlen;
    }
    None
}

fn skip_dns_name(packet: &[u8], offset: &mut usize) -> Option<()> {
    loop {
        let len = *packet.get(*offset)?;
        *offset += 1;
        if len == 0 {
            return Some(());
        }
        if len & 0xc0 == 0xc0 {
            *offset += 1;
            return if *offset <= packet.len() {
                Some(())
            } else {
                None
            };
        }
        *offset += len as usize;
        if *offset > packet.len() {
            return None;
        }
    }
}

fn parse_url(input: &str) -> Result<UrlParts, String> {
    let scheme_end = input
        .find("://")
        .ok_or_else(|| String::from("URL must include a scheme"))?;
    let scheme = input[..scheme_end].to_ascii_lowercase();
    let rest = &input[scheme_end + 3..];
    let path_start = match (rest.find('/'), rest.find('?')) {
        (Some(slash), Some(query)) => slash.min(query),
        (Some(slash), None) => slash,
        (None, Some(query)) => query,
        (None, None) => rest.len(),
    };
    let authority = &rest[..path_start];
    let path = if path_start < rest.len() {
        if rest.as_bytes()[path_start] == b'?' {
            format!("/{}", &rest[path_start..])
        } else {
            rest[path_start..].to_string()
        }
    } else {
        String::from("/")
    };
    if authority.is_empty() {
        return Err(String::from("URL host is empty"));
    }

    let (host, port) = if let Some(colon) = authority.rfind(':') {
        let host = &authority[..colon];
        let port =
            parse_u16(&authority[colon + 1..]).ok_or_else(|| String::from("invalid URL port"))?;
        (host.to_string(), port)
    } else {
        let port = match scheme.as_str() {
            "http" => HTTP_PORT,
            "https" => HTTPS_PORT,
            _ => return Err(format!("unsupported URL scheme: {}", scheme)),
        };
        (authority.to_string(), port)
    };

    Ok(UrlParts {
        scheme,
        host,
        port,
        path,
    })
}

fn form_value(input: &str, name: &str) -> Option<String> {
    for pair in input.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = form_decode(parts.next().unwrap_or(""))?;
        if key != name {
            continue;
        }
        return form_decode(parts.next().unwrap_or(""));
    }
    None
}

fn form_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = String::new();
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(' ');
                index += 1;
            }
            b'%' => {
                let high = *bytes.get(index + 1)?;
                let low = *bytes.get(index + 2)?;
                let value = (hex_digit(high)? << 4) | hex_digit(low)?;
                out.push(value as char);
                index += 3;
            }
            byte => {
                out.push(byte as char);
                index += 1;
            }
        }
    }

    Some(out)
}

fn append_query_param(url: &mut String, name: &str, value: &str) {
    if url.contains('?') {
        url.push('&');
    } else {
        url.push('?');
    }
    url.push_str(name);
    url.push('=');
    push_form_encoded(url, value);
}

fn push_form_encoded(out: &mut String, input: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn resolve_redirect(base: &UrlParts, location: &str) -> Result<UrlParts, String> {
    if location.contains("://") {
        return parse_url(location);
    }
    if location.starts_with('/') {
        return Ok(UrlParts {
            scheme: base.scheme.clone(),
            host: base.host.clone(),
            port: base.port,
            path: location.to_string(),
        });
    }
    let mut path = base.path.clone();
    if let Some(slash) = path.rfind('/') {
        path.truncate(slash + 1);
    }
    path.push_str(location);
    Ok(UrlParts {
        scheme: base.scheme.clone(),
        host: base.host.clone(),
        port: base.port,
        path,
    })
}

fn parse_status(headers: &str) -> Result<u16, String> {
    let first = headers
        .lines()
        .next()
        .ok_or_else(|| String::from("empty HTTP response"))?;
    let mut parts = first.split_whitespace();
    let _version = parts.next();
    let status = parts
        .next()
        .and_then(parse_u16)
        .ok_or_else(|| String::from("invalid HTTP status"))?;
    Ok(status)
}

fn header_value(headers: &str, name: &str) -> Option<String> {
    for line in headers.lines().skip(1) {
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = &line[..colon];
        if eq_ignore_ascii_case(key, name) {
            return Some(line[colon + 1..].trim().to_string());
        }
    }
    None
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn wait_readable(socket: &Socket, timeout_ns: i64) -> Result<(), String> {
    let mut poll_handle = [PollHandle::new(socket.as_raw() as u32, POLLIN)];
    match poll(&mut poll_handle, timeout_ns) {
        Ok(0) => Err(String::from("network read timed out")),
        Ok(_) => Ok(()),
        Err(_) => Err(String::from("network poll failed")),
    }
}

fn write_all<W: Write>(writer: &mut W, mut data: &[u8], context: &str) -> Result<(), String> {
    let mut would_block_retries = 0usize;
    let max_would_block_retries = if context == "media stream" { 6000 } else { 200 };
    while !data.is_empty() {
        let n = match writer.write(data) {
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                would_block_retries += 1;
                if would_block_retries > max_would_block_retries {
                    return Err(format!("{} write timed out", context));
                }
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(err) => return Err(format!("{} write failed: {}", context, err)),
        };
        if n == 0 {
            return Err(format!("{} short write", context));
        }
        would_block_retries = 0;
        data = &data[n..];
        if !data.is_empty() {
            thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}

fn is_youtube_url(input: &str) -> bool {
    input.contains("youtube.com/") || input.contains("youtu.be/")
}

fn looks_like_url(input: &str) -> bool {
    input.contains("://")
}

fn youtube_video_id(input: &str) -> Option<&str> {
    if let Some(pos) = input.find("youtu.be/") {
        let tail = &input[pos + "youtu.be/".len()..];
        return Some(tail.split(['?', '&', '/']).next()?);
    }
    let query = input.split('?').nth(1)?;
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("v=") {
            return Some(value);
        }
    }
    None
}

fn parse_ipv4(input: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut count = 0usize;
    for part in input.split('.') {
        if count >= 4 {
            return None;
        }
        out[count] = parse_u8(part)?;
        count += 1;
    }
    if count == 4 { Some(out) } else { None }
}

fn parse_u8(input: &str) -> Option<u8> {
    let value = parse_usize(input)?;
    if value <= u8::MAX as usize {
        Some(value as u8)
    } else {
        None
    }
}

fn parse_u16(input: &str) -> Option<u16> {
    let value = parse_usize(input)?;
    if value <= u16::MAX as usize {
        Some(value as u16)
    } else {
        None
    }
}

fn parse_usize(input: &str) -> Option<usize> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0usize;
    for byte in input.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as usize)?;
    }
    Some(value)
}

fn parse_hex_usize(input: &str) -> Option<usize> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0usize;
    for byte in input.bytes() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(digit as usize)?;
    }
    Some(value)
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
    ]))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    find_bytes_from(haystack, needle, 0)
}

fn find_bytes_from(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() || start > haystack.len() {
        return None;
    }
    (start..=haystack.len() - needle.len())
        .find(|&index| &haystack[index..index + needle.len()] == needle)
}

fn eq_ignore_ascii_case(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .all(|(a, b)| a.eq_ignore_ascii_case(&b))
}
