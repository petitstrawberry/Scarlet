//! Own the local seat until the session supervisor admits a desktop.
//! The public compositor sockets and input workers are created after this gate.
mod greeter_ui;
#[allow(dead_code)]
#[path = "soft_keyboard/layout.rs"]
mod keyboard_layout;
mod seat_keys;
mod seat_pointer;
use greeter_ui::Screen;
use keyboard_layout::Layer;
use seat_keys::key_byte;

use framebuffer::DisplaySurface;
use scarlet_os::input::{INPUT_CAP_MT, InputDeviceKind};
use scarlet_os::{handle::capability::StreamError, input::InputDevice};
use std::{
    io::{self, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

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

struct PointerDevice {
    device: InputDevice,
    pointer: seat_pointer::Pointer,
}
impl PointerDevice {
    fn open(path: &str) -> Option<Self> {
        let device = InputDevice::open(path).ok()?;
        device.set_nonblocking(true).ok()?;
        let kind = device.kind().ok()?;
        let mt = device.capabilities().ok()? & INPUT_CAP_MT != 0;
        let axes = if matches!(kind, InputDeviceKind::Touchscreen | InputDeviceKind::Tablet)
            || mt
            || device.absolute_axis(0).is_ok()
        {
            let x = device.absolute_axis(if mt { 53 } else { 0 }).ok()?;
            let y = device.absolute_axis(if mt { 54 } else { 1 }).ok()?;
            Some((
                seat_pointer::Axis {
                    min: x.minimum,
                    max: x.maximum,
                },
                seat_pointer::Axis {
                    min: y.minimum,
                    max: y.maximum,
                },
            ))
        } else if kind == InputDeviceKind::Mouse {
            None
        } else {
            return None;
        };
        Some(Self {
            device,
            pointer: seat_pointer::Pointer::new(axes, mt),
        })
    }
    fn read_tap(&mut self, w: u32, h: u32) -> Result<Option<(i32, i32)>, ()> {
        let mut record = [0; 16];
        // Keep discovery, keyboard input and display changes responsive.
        for _ in 0..128 {
            match self.device.read(&mut record) {
                Ok(16) => {}
                Ok(_) | Err(StreamError::WouldBlock | StreamError::Interrupted) => return Ok(None),
                _ => return Err(()),
            }
            let kind = u16::from_ne_bytes(record[8..10].try_into().unwrap());
            let code = u16::from_ne_bytes(record[10..12].try_into().unwrap());
            let value = i32::from_ne_bytes(record[12..16].try_into().unwrap());

            if let Some(tap) = self.pointer.event(kind, code, value, w, h) {
                return Ok(Some(tap));
            }
        }
        Ok(None)
    }
}

// Use a bounded logical viewport on high density panels and small display
// modes. The pointer decoder normalizes to these same logical dimensions.
fn viewport(width: u32, height: u32) -> io::Result<(u32, u32)> {
    if width == 0 || height == 0 {
        return Err(io::Error::other("empty login display"));
    }
    let scale = (1440.0 / width.max(height) as f64)
        .min(1.0)
        .max(320.0 / width as f64)
        .max(360.0 / height as f64);
    Ok((
        ((width as f64 * scale).round() as u32).clamp(320, 1440),
        ((height as f64 * scale).round() as u32).clamp(360, 1440),
    ))
}
fn paint(
    display: &mut DisplaySurface,
    screen: &Screen,
    pixels: &[u8],
    output: (u32, u32),
) -> io::Result<()> {
    let (w, h) = output;
    let err = |e| io::Error::other(format!("cannot write login display: {e:?}"));
    if output == (screen.width, screen.height) {
        display
            .write_bgra_strided(0, 0, w, h, pixels, w as usize * 4)
            .map_err(err)?;
    } else {
        // Expand one scanline at a time, without another full-size framebuffer.
        let columns: Vec<usize> = (0..w)
            .map(|x| (u64::from(x) * u64::from(screen.width) / u64::from(w)) as usize * 4)
            .collect();
        let mut line = vec![0; w as usize * 4];
        let mut last = None;
        for y in 0..h {
            let row = (u64::from(y) * u64::from(screen.height) / u64::from(h)) as usize;
            if last != Some(row) {
                let src = &pixels[row * screen.width as usize * 4..];
                for (dst, &x) in line.chunks_exact_mut(4).zip(&columns) {
                    dst.copy_from_slice(&src[x..x + 4]);
                }
                last = Some(row);
            }
            display
                .write_bgra_strided(0, y, w, 1, &line, line.len())
                .map_err(err)?;
        }
    }
    display
        .present()
        .map_err(|e| io::Error::other(format!("cannot present login screen: {e:?}")))
}

#[path = "scarlet_shell/fonts.rs"]
mod fonts;

fn authenticate(broker: &mut Worker, config: &Config) -> io::Result<()> {
    let mut renderer = Worker::start(&config.broker, &["--render-greeter", &config.greeter])?;
    let mut display = DisplaySurface::open_primary()
        .map_err(|_| io::Error::other("login display unavailable"))?;
    // The trusted copy must use the same font metrics for View hit testing.
    fonts::initialize()?;
    let mut panel_input = seat_keys::PanelInput::new();
    let mut keys: [Keyboard; 8] = std::array::from_fn(|_| Keyboard::default());
    let mut pointers: [Option<PointerDevice>; 24] = std::array::from_fn(|_| None);
    let mut scan = None;
    let mut state = [0; 37];
    broker.exchange(&[19], &mut state)?;
    let info = display
        .get_info()
        .map_err(|_| io::Error::other("login display info"))?;
    let mut output = (info.width, info.height);
    let (width, height) = viewport(output.0, output.1)?;
    let mut screen = Screen {
        width,
        height,
        state: state[1..].try_into().unwrap(),
        keyboard: true,
        shift: false,
        layer: Layer::Letters,
    };
    let mut pixels = Vec::new();
    loop {
        // Validate dimensions before either peer allocates a framebuffer.
        Screen::decode(&screen.encode())?;
        let hit_test = screen.targets();
        pixels.resize(screen.pixel_bytes(), 0);
        renderer.exchange(&screen.encode(), &mut pixels)?;
        paint(&mut display, &screen, &pixels, output)?;
        loop {
            let info = display
                .get_info()
                .map_err(|_| io::Error::other("login display info"))?;
            if (info.width, info.height) != output {
                display = DisplaySurface::open_primary()
                    .map_err(|_| io::Error::other("reopen login display"))?;
                output = (info.width, info.height);
                (screen.width, screen.height) = viewport(output.0, output.1)?;
                for pointer in pointers.iter_mut().flatten() {
                    pointer.pointer.cancel();
                }
                break;
            }
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
                for (index, pointer) in pointers.iter_mut().enumerate() {
                    if pointer.is_none() {
                        let prefix = ["touchscreen", "mouse", "tablet"][index / 8];
                        *pointer = PointerDevice::open(&format!("/dev/{prefix}{}", index % 8));
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
            let mut action = None;
            if pressed.is_none() {
                for pointer in &mut pointers {
                    let Some(device) = pointer else { continue };
                    match device.read_tap(screen.width, screen.height) {
                        Ok(Some((x, y))) => {
                            action = hit_test.at(x, y);
                            break;
                        }
                        Ok(None) => {}
                        Err(()) => *pointer = None,
                    }
                }
            }
            let redraw = action.is_some();
            if let Some(action) = action {
                pressed = panel_input.activate(&mut screen, action);
            }
            if let Some(byte) = pressed {
                broker.exchange(&[byte], &mut state)?;
                if state[0] == 2 {
                    return Ok(());
                }
                screen.state.copy_from_slice(&state[1..]);
                // Do not carry a contact into a changed field or auth result.
                for pointer in pointers.iter_mut().flatten() {
                    pointer.pointer.cancel();
                }
                break;
            }
            if redraw {
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
