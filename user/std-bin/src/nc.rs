//! Transfer data over TCP or UDP connections.

use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream, ToSocketAddrs,
    UdpSocket,
};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const BUFFER_SIZE: usize = 16 * 1024;
const UDP_POLL_INTERVAL: Duration = Duration::from_millis(250);
const UDP_EOF_GRACE_POLLS: usize = 4;

fn main() -> ExitCode {
    let options = match parse_args(env::args().skip(1)) {
        Ok(ParseResult::Run(options)) => options,
        Ok(ParseResult::Help) => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("nc: {err}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("nc: {err}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    listen: bool,
    udp: bool,
    numeric_only: bool,
    verbose: bool,
    zero_io: bool,
    timeout: Option<Duration>,
    host: String,
    port: u16,
}

enum ParseResult {
    Run(Options),
    Help,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<ParseResult, String> {
    let mut listen = false;
    let mut udp = false;
    let mut numeric_only = false;
    let mut verbose = false;
    let mut zero_io = false;
    let mut timeout = None;
    let mut positional = Vec::new();
    let mut parsing_options = true;
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        if parsing_options && arg == "--" {
            parsing_options = false;
            continue;
        }
        if parsing_options && (arg == "-h" || arg == "--help") {
            return Ok(ParseResult::Help);
        }
        if parsing_options && (arg == "-w" || arg == "--wait") {
            let value = args
                .next()
                .ok_or_else(|| "option requires an argument: -w".to_owned())?;
            timeout = Some(parse_timeout(&value)?);
            continue;
        }
        if parsing_options && arg.starts_with('-') && arg.len() > 1 {
            if arg.starts_with("--") {
                return Err(format!("unknown option: {arg}"));
            }
            for option in arg[1..].chars() {
                match option {
                    'l' => listen = true,
                    'u' => udp = true,
                    'n' => numeric_only = true,
                    'v' => verbose = true,
                    'z' => zero_io = true,
                    'h' => return Ok(ParseResult::Help),
                    'w' => {
                        return Err("-w must be followed by a separate timeout value".to_owned());
                    }
                    _ => return Err(format!("unknown option: -{option}")),
                }
            }
            continue;
        }
        positional.push(arg);
    }

    let (host, port) = if listen {
        match positional.as_slice() {
            [port] => (Ipv4Addr::UNSPECIFIED.to_string(), parse_port(port)?),
            [host, port] => (host.clone(), parse_port(port)?),
            _ => return Err("listen mode requires [host] port".to_owned()),
        }
    } else {
        match positional.as_slice() {
            [host, port] => (host.clone(), parse_port(port)?),
            _ => return Err("connect mode requires host and port".to_owned()),
        }
    };

    Ok(ParseResult::Run(Options {
        listen,
        udp,
        numeric_only,
        verbose,
        zero_io,
        timeout,
        host,
        port,
    }))
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("invalid port: {value}"))
}

fn parse_timeout(value: &str) -> Result<Duration, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds != 0)
        .map(Duration::from_secs)
        .ok_or_else(|| format!("invalid timeout: {value}"))
}

fn run(options: &Options) -> io::Result<()> {
    match (options.listen, options.udp) {
        (false, false) => run_tcp_client(options),
        (true, false) => run_tcp_listener(options),
        (false, true) => run_udp_client(options),
        (true, true) => run_udp_listener(options),
    }
}

fn run_tcp_client(options: &Options) -> io::Result<()> {
    let addrs = resolve(&options.host, options.port, options.numeric_only)?;
    let stream = connect_tcp(&addrs, options.timeout)?;
    if options.verbose {
        eprintln!("nc: connected to {} {} (tcp)", options.host, options.port);
    }
    if options.zero_io {
        return Ok(());
    }
    configure_tcp(&stream, options.timeout)?;
    relay_tcp(stream)
}

fn run_tcp_listener(options: &Options) -> io::Result<()> {
    let addr = resolve_one(&options.host, options.port, options.numeric_only)?;
    let listener = TcpListener::bind(addr)?;
    if options.verbose {
        eprintln!("nc: listening on {addr} (tcp)");
    }
    let (stream, peer) = listener.accept()?;
    if options.verbose {
        eprintln!("nc: connection from {peer}");
    }
    if options.zero_io {
        return Ok(());
    }
    configure_tcp(&stream, options.timeout)?;
    relay_tcp(stream)
}

fn configure_tcp(stream: &TcpStream, timeout: Option<Duration>) -> io::Result<()> {
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)
}

fn connect_tcp(addrs: &[SocketAddr], timeout: Option<Duration>) -> io::Result<TcpStream> {
    let mut last_error = None;
    for addr in addrs {
        let result = match timeout {
            Some(timeout) => TcpStream::connect_timeout(addr, timeout),
            None => TcpStream::connect(addr),
        };
        match result {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::new(ErrorKind::NotFound, "no usable address")))
}

fn relay_tcp(stream: TcpStream) -> io::Result<()> {
    let stream = Arc::new(stream);
    let writer = Arc::clone(&stream);
    let _input_thread = thread::spawn(move || send_stdin_tcp(writer));
    let mut reader = stream.as_ref();
    let mut stdout = io::stdout();
    let mut buffer = [0; BUFFER_SIZE];

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                stdout.write_all(&buffer[..count])?;
                stdout.flush()?;
            }
            Err(err) if is_timeout(&err) => break,
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn send_stdin_tcp(stream: Arc<TcpStream>) {
    let mut stdin = io::stdin();
    let mut writer = stream.as_ref();
    let result = copy_stream(&mut stdin, &mut writer);
    let _ = stream.shutdown(Shutdown::Write);
    if let Err(err) = result {
        eprintln!("nc: stdin send failed: {err}");
    }
}

fn run_udp_client(options: &Options) -> io::Result<()> {
    let remote = resolve_one(&options.host, options.port, options.numeric_only)?;
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect(remote)?;
    if options.verbose {
        eprintln!("nc: connected to {remote} (udp)");
    }
    if options.zero_io {
        socket.send(&[])?;
        return Ok(());
    }
    relay_udp(socket, None, options.timeout)
}

fn run_udp_listener(options: &Options) -> io::Result<()> {
    let addr = resolve_one(&options.host, options.port, options.numeric_only)?;
    let socket = UdpSocket::bind(addr)?;
    socket.set_read_timeout(options.timeout)?;
    if options.verbose {
        eprintln!("nc: listening on {addr} (udp)");
    }

    let mut buffer = [0; BUFFER_SIZE];
    let (count, peer) = socket.recv_from(&mut buffer)?;
    socket.connect(peer)?;
    if options.verbose {
        eprintln!("nc: datagram from {peer}");
    }
    if options.zero_io {
        return Ok(());
    }
    relay_udp(socket, Some(&buffer[..count]), options.timeout)
}

fn relay_udp(
    socket: UdpSocket,
    first_packet: Option<&[u8]>,
    timeout: Option<Duration>,
) -> io::Result<()> {
    let poll_timeout = timeout.unwrap_or(UDP_POLL_INTERVAL);
    socket.set_read_timeout(Some(poll_timeout))?;
    socket.set_write_timeout(timeout)?;

    let socket = Arc::new(socket);
    let input_done = Arc::new(AtomicBool::new(false));
    let writer = Arc::clone(&socket);
    let writer_done = Arc::clone(&input_done);
    let _input_thread = thread::spawn(move || send_stdin_udp(writer, writer_done));

    let mut stdout = io::stdout();
    if let Some(packet) = first_packet {
        stdout.write_all(packet)?;
        stdout.flush()?;
    }

    let mut buffer = [0; BUFFER_SIZE];
    let mut eof_polls = 0;
    loop {
        match socket.recv(&mut buffer) {
            Ok(count) => {
                stdout.write_all(&buffer[..count])?;
                stdout.flush()?;
                eof_polls = 0;
            }
            Err(err) if is_timeout(&err) => {
                if timeout.is_some() {
                    break;
                }
                if input_done.load(Ordering::Acquire) {
                    eof_polls += 1;
                    if eof_polls >= UDP_EOF_GRACE_POLLS {
                        break;
                    }
                }
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn send_stdin_udp(socket: Arc<UdpSocket>, input_done: Arc<AtomicBool>) {
    let mut stdin = io::stdin();
    let mut buffer = [0; BUFFER_SIZE];
    loop {
        match stdin.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => match socket.send(&buffer[..count]) {
                Ok(sent) if sent == count => {}
                Ok(_) => {
                    eprintln!("nc: partial UDP datagram send");
                    break;
                }
                Err(err) => {
                    eprintln!("nc: stdin send failed: {err}");
                    break;
                }
            },
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("nc: stdin read failed: {err}");
                break;
            }
        }
    }
    input_done.store(true, Ordering::Release);
}

fn copy_stream(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    let mut buffer = [0; BUFFER_SIZE];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return writer.flush(),
            Ok(count) => writer.write_all(&buffer[..count])?,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

fn resolve_one(host: &str, port: u16, numeric_only: bool) -> io::Result<SocketAddr> {
    resolve(host, port, numeric_only)?
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "no usable IPv4 address"))
}

fn resolve(host: &str, port: u16, numeric_only: bool) -> io::Result<Vec<SocketAddr>> {
    let addrs = if numeric_only {
        let ip = host.parse::<IpAddr>().map_err(|_| {
            io::Error::new(ErrorKind::InvalidInput, "numeric address required by -n")
        })?;
        vec![SocketAddr::new(ip, port)]
    } else {
        (host, port).to_socket_addrs()?.collect()
    };
    let addrs = addrs
        .into_iter()
        .filter(SocketAddr::is_ipv4)
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "IPv6 is not supported",
        ))
    } else {
        Ok(addrs)
    }
}

fn is_timeout(err: &io::Error) -> bool {
    matches!(err.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

fn print_usage() {
    eprintln!("usage: nc [-lnuvz] [-w seconds] host port");
    eprintln!("       nc -l [-nuvz] [-w seconds] [host] port");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Options {
        match parse_args(args.iter().map(|arg| (*arg).to_owned())).unwrap() {
            ParseResult::Run(options) => options,
            ParseResult::Help => panic!("unexpected help"),
        }
    }

    #[test]
    fn parses_client_options() {
        let options = parse(&["-unvz", "-w", "3", "127.0.0.1", "53"]);
        assert!(options.udp);
        assert!(options.numeric_only);
        assert!(options.verbose);
        assert!(options.zero_io);
        assert_eq!(options.timeout, Some(Duration::from_secs(3)));
        assert_eq!(options.host, "127.0.0.1");
        assert_eq!(options.port, 53);
    }

    #[test]
    fn listen_defaults_to_unspecified_address() {
        let options = parse(&["-l", "8080"]);
        assert!(options.listen);
        assert_eq!(options.host, "0.0.0.0");
        assert_eq!(options.port, 8080);
    }

    #[test]
    fn rejects_invalid_port() {
        assert!(parse_args(["localhost", "0"].into_iter().map(str::to_owned)).is_err());
    }
}
