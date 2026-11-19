//! Own the physical seat until the session supervisor admits a desktop.
//! The public compositor sockets and input workers are created after this gate.
use framebuffer::DisplaySurface;
use scarlet_os::{handle::capability::StreamError, input::InputDevice};
use std::{
    io::{self, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 240;

struct Worker {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
}

impl Worker {
    fn start(program: &str, args: &[&str]) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            input: child.stdin.take(),
            output: child.stdout.take().unwrap(),
            child,
        })
    }

    fn exchange(&mut self, request: &[u8], reply: &mut [u8]) -> io::Result<()> {
        self.input.as_mut().unwrap().write_all(request)?;
        self.output.read_exact(reply)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // EOF makes the broker terminate its desktop scope. Reap the greeter too.
        drop(self.input.take());
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct Keyboard {
    device: Option<InputDevice>,
    shifts: u8,
    caps: bool,
    desynced: bool,
}

impl Keyboard {
    fn read_key(&mut self) -> io::Result<Option<u8>> {
        let Some(device) = &self.device else {
            return Ok(None);
        };
        let mut record = [0; 16];
        loop {
            match device.read(&mut record) {
                Ok(16) => {}
                Ok(_) | Err(StreamError::WouldBlock | StreamError::Interrupted) => return Ok(None),
                Err(_) => {
                    *self = Self::default();
                    return Ok(None);
                }
            }
            let kind = u16::from_ne_bytes([record[8], record[9]]);
            let code = u16::from_ne_bytes([record[10], record[11]]);
            let value = i32::from_ne_bytes(record[12..16].try_into().unwrap());
            if kind == 0 && code == 3 {
                self.desynced = true;
                self.shifts = 0;
            }
            if kind == 0 && code == 0 {
                self.desynced = false;
            }
            if self.desynced || kind != 1 {
                continue;
            }
            if matches!(code, 42 | 54) {
                let bit = if code == 42 { 1 } else { 2 };
                if value == 0 {
                    self.shifts &= !bit;
                } else {
                    self.shifts |= bit;
                }
            } else if value == 1 {
                if code == 58 {
                    self.caps = !self.caps;
                } else if let Some(byte) = key_byte(code, self.shifts != 0, self.caps) {
                    return Ok(Some(byte));
                }
            }
        }
    }
}

// US evdev layout for the trusted login prompt. IMEs belong to the desktop.
fn key_byte(code: u16, shift: bool, caps: bool) -> Option<u8> {
    let (normal, shifted, start): (&[u8], &[u8], u16) = match code {
        1 => return Some(27),
        14 => return Some(8),
        28 | 96 => return Some(b'\n'),
        57 => return Some(b' '),
        2..=13 => (b"1234567890-=", b"!@#$%^&*()_+", 2),
        16..=27 => (b"qwertyuiop[]", b"QWERTYUIOP{}", 16),
        30..=41 => (b"asdfghjkl;'\x60", b"ASDFGHJKL:\"~", 30),
        43 => return Some(if shift { b'|' } else { b'\\' }),
        44..=53 => (b"zxcvbnm,./", b"ZXCVBNM<>?", 44),
        _ => return None,
    };
    let index = usize::from(code - start);
    Some(if shift ^ (caps && normal[index].is_ascii_alphabetic()) {
        shifted[index]
    } else {
        normal[index]
    })
}

fn paint(display: &mut DisplaySurface, panel: &[u8]) -> io::Result<()> {
    let info = display
        .get_info()
        .map_err(|_| io::Error::other("login display info"))?;
    let (w, h) = (info.width, info.height);
    let stride = (w as usize)
        .checked_mul(4)
        .ok_or_else(|| io::Error::other("invalid display width"))?;
    let size = stride
        .checked_mul(h as usize)
        .ok_or_else(|| io::Error::other("invalid display height"))?;
    let mut frame = vec![0; size];
    let pw = w.min(WIDTH) as usize;
    let ph = h.min(HEIGHT) as usize;
    let x = (w as usize - pw) / 2;
    let y = (h as usize - ph) / 2;
    for row in 0..ph {
        let dest = (row + y) * stride + x * 4;
        let source = row * WIDTH as usize * 4;
        frame[dest..dest + pw * 4].copy_from_slice(&panel[source..source + pw * 4]);
    }
    display
        .write_bgra_strided(0, 0, w, h, &frame, stride)
        .map_err(|e| io::Error::other(format!("cannot write login display: {e:?}")))?;
    display
        .present()
        .map_err(|e| io::Error::other(format!("cannot present login screen: {e:?}")))
}

fn authenticate(broker: &mut Worker, config: &Config) -> io::Result<()> {
    let mut renderer = Worker::start(&config.broker, &["--render-greeter", &config.greeter])?;
    let mut display = DisplaySurface::open_primary()
        .map_err(|_| io::Error::other("login display unavailable"))?;
    let mut keys: [Keyboard; 8] = std::array::from_fn(|_| Keyboard::default());
    let mut scan = None;
    let mut pixels = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    let mut state = [0; 37];
    broker.exchange(&[0], &mut state)?;
    loop {
        renderer.exchange(&state[1..], &mut pixels)?;
        paint(&mut display, &pixels)?;
        loop {
            let now = Instant::now();
            if scan.is_none_or(|last: Instant| now.duration_since(last) >= Duration::from_secs(1)) {
                scan = Some(now);
                for (index, kbd) in keys.iter_mut().enumerate() {
                    if kbd.device.is_none() {
                        kbd.device = InputDevice::open(&format!("/dev/keyboard{index}"))
                            .ok()
                            .filter(|d| d.set_nonblocking(true).is_ok());
                    }
                }
            }
            let mut pressed = None;
            for keyboard in &mut keys {
                if let Some(byte) = keyboard.read_key()? {
                    pressed = Some(byte);
                    break;
                }
            }
            if let Some(byte) = pressed {
                broker.exchange(&[byte], &mut state)?;
                if state[0] == 2 {
                    return Ok(());
                }
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    broker: String,
    greeter: String,
    compositor: Vec<String>,
    display_socket: String,
    session: Vec<String>,
}

fn run_session(config: &Config) -> io::Result<()> {
    let mut args = vec!["--session"];
    args.extend(config.session.iter().map(String::as_str));
    let mut broker = Worker::start(&config.broker, &args)?;
    authenticate(&mut broker, config)?;
    let mut compositor = Command::new(&config.compositor[0])
        .args(&config.compositor[1..])
        .stdin(Stdio::null())
        .spawn()?;
    let result = (|| {
        let mut state = [0; 37];
        let start = Instant::now();
        while !sws_client::Connection::connect(&config.display_socket)
            .is_ok_and(|conn| conn.get_screen_size().is_ok())
        {
            if compositor.try_wait()?.is_some() || start.elapsed() >= Duration::from_secs(30) {
                return Err(io::Error::other("compositor did not become ready"));
            }
            thread::sleep(Duration::from_millis(50));
        }
        broker.exchange(&[16], &mut state)?;
        while state[0] == 1 && compositor.try_wait()?.is_none() {
            thread::sleep(Duration::from_millis(200));
            broker.exchange(&[0], &mut state)?;
        }
        Ok(())
    })();
    let _ = compositor.kill();
    let _ = compositor.wait();
    result
}

fn main() -> io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| io::Error::other("expected seat configuration path"))?;
    let config: Config =
        toml::from_str(&std::fs::read_to_string(path)?).map_err(io::Error::other)?;
    if ![&config.broker, &config.greeter, &config.display_socket]
        .into_iter()
        .all(|s| s.starts_with('/'))
        || [&config.compositor, &config.session]
            .into_iter()
            .any(|argv| !argv.first().is_some_and(|s| s.starts_with('/')))
    {
        return Err(io::Error::other(
            "expected absolute seat programs and socket",
        ));
    }
    loop {
        if let Err(e) = run_session(&config) {
            eprintln!("[scarlet-seat] {e}");
            thread::sleep(Duration::from_secs(1));
        }
    }
}
