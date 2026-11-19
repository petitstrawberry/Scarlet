//! The seat starts a private SWS instance and the system input panel before
//! authentication. The ordinary desktop starts after those processes have exited.
#[path = "scarlet_shell/clock.rs"]
mod clock;
#[path = "scarlet_shell/fonts.rs"]
mod fonts;
mod greeter_ui;
use scarlet_os::{
    Socket,
    ipc::{EventPriority, ProcessControl, send_process_control_with_priority},
};
use scarlet_ui::{BitmapImage, RenderingPipeline, prelude::*};
use std::{
    fs::File,
    io::{self, Read, Write},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Worker {
    child: Child,
    input: Option<File>,
    output: File,
}

impl Worker {
    fn start(program: &str, args: &[&str]) -> io::Result<Self> {
        let (read_request, write_request) = private_pipe()?;
        let (read_reply, write_reply) = private_pipe()?;
        let child = Command::new(program)
            .args(args)
            .env_clear()
            .stdin(child_stdio(read_request)?)
            .stdout(child_stdio(write_reply)?)
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            input: Some(file_from_handle(write_request)),
            output: file_from_handle(read_reply),
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

fn os_err(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{e:?}"))
}

fn set_close_on_exec(handle: &scarlet_os::Handle, enabled: bool) -> io::Result<()> {
    // SAFETY: set the descriptor-local close-on-exec bit on an owned handle.
    let result = unsafe {
        scarlet_sys::syscall2(
            scarlet_sys::Syscall::HandleSetDescriptorFlags,
            handle.as_raw() as usize,
            enabled as usize,
        )
    };
    if result != 0 {
        return Err(io::Error::other("cannot isolate login descriptor"));
    }
    Ok(())
}
fn file_from_handle(handle: scarlet_os::Handle) -> File {
    use std::os::fd::FromRawFd;
    let fd = std::mem::ManuallyDrop::new(handle);
    // SAFETY: transfer sole ownership of this descriptor into the std File.
    unsafe { File::from_raw_fd(fd.as_raw()) }
}
// Scarlet std duplicates the descriptor metadata when assigning child stdio.
// Clear CLOEXEC only on the endpoints handed to this spawn. The seat spawns
// children serially, dropping the Command and its endpoints before the next.
fn child_stdio(handle: scarlet_os::Handle) -> io::Result<File> {
    set_close_on_exec(&handle, false)?;
    Ok(file_from_handle(handle))
}

fn private_pipe() -> io::Result<(scarlet_os::Handle, scarlet_os::Handle)> {
    let (read, write) = scarlet_os::ipc::pipe().map_err(os_err)?;
    set_close_on_exec(&read, true)?;
    set_close_on_exec(&write, true)?;
    Ok((read, write))
}
fn private_pair() -> io::Result<(Socket, Socket)> {
    let (a, b) = Socket::pair().map_err(os_err)?;
    set_close_on_exec(a.as_handle(), true)?;
    set_close_on_exec(b.as_handle(), true)?;
    Ok((a, b))
}

struct SeatChild {
    child: Child,
    reaped: bool,
}

impl SeatChild {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        Ok(Self {
            child: command.spawn()?,
            reaped: false,
        })
    }

    fn with_login_socket(command: &mut Command, input: Socket) -> io::Result<Self> {
        let file = child_stdio(input.into_handle())?;
        Self::spawn(
            command
                .env_clear()
                .env("SCARLET_UI_BACKEND", "cpu")
                .env("SWS_BACKEND", "cpu")
                .stdin(Stdio::from(file))
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
    }

    fn exited(&mut self) -> io::Result<bool> {
        if !self.reaped {
            self.reaped = self.child.try_wait()?.is_some();
        }
        Ok(self.reaped)
    }

    fn running(&mut self) -> io::Result<()> {
        if self.exited()? {
            return Err(io::Error::other("login display service exited"));
        }
        Ok(())
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.exited()? {
            return Ok(());
        }
        // Scarlet std's Child::kill currently returns Unsupported. Stop the
        // display owner through native process control before waiting for it.
        if let Err(error) = send_process_control_with_priority(
            self.child.id(),
            ProcessControl::Kill,
            EventPriority::Critical,
            true,
        ) {
            return if self.exited()? {
                Ok(())
            } else {
                Err(os_err(error))
            };
        }
        self.child.wait()?;
        self.reaped = true;
        Ok(())
    }
}

impl Drop for SeatChild {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!(
                "[scarlet-seat] could not stop child {}: {error}",
                self.child.id()
            );
        }
    }
}

fn authenticate(broker: &mut Worker, config: &Config) -> io::Result<()> {
    let (editor, editor_server) = private_pair()?;
    let (provider, provider_server) = private_pair()?;
    let (bootstrap, child_socket) = private_pair()?;
    bootstrap
        .send_handle(editor_server.as_handle())
        .map_err(os_err)?;
    bootstrap
        .send_handle(provider_server.as_handle())
        .map_err(os_err)?;
    drop((editor_server, provider_server));
    let mut compositor = SeatChild::with_login_socket(
        Command::new(&config.compositor[0])
            .arg("--login-seat")
            .args(&config.compositor[1..]),
        child_socket,
    )?;
    let mut keyboard =
        SeatChild::with_login_socket(Command::new(&config.broker).arg("--login-panel"), provider)?;
    let mut painter = Worker::start(&config.broker, &["--render-greeter", &config.greeter])?;
    let result = sws_client::Connection::with_inherited_socket(editor, || {
        login_window(broker, &mut painter, || {
            compositor.running()?;
            keyboard.running()
        })
    })
    .map_err(os_err)
    .and_then(|result| result);
    let stop_keyboard = keyboard.stop();
    let stop_compositor = compositor.stop();
    result.and(stop_keyboard).and(stop_compositor)
}

fn background(painter: &mut Worker, width: u32, height: u32) -> io::Result<BitmapImage> {
    let ratio = 1440.0 / (width.max(height).max(1) as f64);
    let w = ((width as f64 * ratio.min(1.)).round() as u32).max(1);
    let h = ((height as f64 * ratio.min(1.)).round() as u32).max(1);
    let mut request = [0; 8];
    request[..4].copy_from_slice(&w.to_le_bytes());
    request[4..].copy_from_slice(&h.to_le_bytes());
    let mut pixels = vec![0; w as usize * h as usize * 4];
    painter.exchange(&request, &mut pixels)?;
    Ok(BitmapImage::from_bgra(
        pixels
            .chunks_exact(4)
            .map(|p| u32::from_le_bytes(p.try_into().unwrap()))
            .collect(),
        w,
        h,
    ))
}

#[cfg(target_os = "scarlet")]
fn login_window(
    broker: &mut Worker,
    painter: &mut Worker,
    alive: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    use scarlet_ui::{PlatformWindow, SWSPlatformWindow};
    let conn = sws_client::Connection::connect_default().map_err(os_err)?;
    let (w, h) = conn.get_screen_size().map_err(os_err)?;
    let scale = conn.get_output_scale().map_err(os_err)?.max(1000) as f32 / 1000.;
    let mut window = SWSPlatformWindow::create_with_type(
        "org.scarlet-os.shell.login",
        "Sign in",
        Size::new(w as f32 / scale, h as f32 / scale),
        sws_protocol::window_types::NORMAL,
    )
    .map_err(os_err)?;
    window.set_fullscreen(true).map_err(os_err)?;
    fonts::initialize()?;
    let wait = conn.clone();
    run_editor(broker, painter, &mut window, alive, move || {
        wait.wait_for_subscribed_window_events(Duration::from_millis(80))
            .map(|_| ())
            .map_err(os_err)
    })
}
#[cfg(not(target_os = "scarlet"))]
fn login_window(
    _: &mut Worker,
    _: &mut Worker,
    _: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    Err(io::Error::other("login requires the Scarlet window server"))
}

fn run_editor(
    broker: &mut Worker,
    painter: &mut Worker,
    window: &mut dyn scarlet_ui::PlatformWindow,
    mut alive: impl FnMut() -> io::Result<()>,
    mut wait: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    use greeter_ui::{Actions, Screen};
    use scarlet_ui::event::Event;
    let mut reply = [0; 37];
    broker.exchange(&[19], &mut reply)?;
    let mut size = window.size();
    let mut screen = Screen {
        width: size.width as u32,
        height: size.height as u32,
        covered: 0.,
        display: reply[1..].try_into().unwrap(),
    };
    let actions = Actions::default();
    let value = State::new(scarlet_ui::generate_state_id(), screen.value());
    let mut pipeline = RenderingPipeline::new();
    let clock_format = scarlet_desktop_config::load_desktop_config()
        .status
        .clock_format;
    let time_label = State::new(scarlet_ui::generate_state_id(), String::new());
    let mut next_clock_check = Instant::now();
    let mut wallpaper = background(painter, screen.width, screen.height)?;
    let mut rebuild = true;
    let mut focus = true;
    loop {
        alive()?;
        if screen.display[0] >= 3 && Instant::now() >= next_clock_check {
            next_clock_check = Instant::now() + Duration::from_secs(1);
            if let Some(seconds) = clock::local_seconds_of_day() {
                let label = clock::format_clock(
                    (seconds / 3600) as u8,
                    ((seconds / 60) % 60) as u8,
                    clock_format,
                );
                if time_label.get() != label {
                    time_label.set(label);
                    pipeline.request_redraw();
                }
            }
        }
        if rebuild {
            pipeline.set_root(
                screen
                    .window(
                        actions.clone(),
                        wallpaper.clone(),
                        value.clone(),
                        focus,
                        time_label.clone(),
                    )
                    .create_element(),
            );
            pipeline.layout_initial();
            pipeline.resize(size);
            pipeline.set_scale_milli(window.output_scale_milli());
            rebuild = false;
        }
        // Same focused TextField contract as the ScarletUI ApplicationRunner.
        window.sync_text_input(pipeline.focused_text_input_state().as_ref());
        if pipeline.has_dirty()
            && let Some(frame) = pipeline.render()
        {
            window.present(frame);
        }
        wait()?;
        while let Some(event) = window.poll_event() {
            let key = match &event {
                Event::Quit | Event::Window(scarlet_ui::event::WindowEvent::CloseRequested) => {
                    return Err(io::Error::other("login surface closed"));
                }
                Event::Resize { width, height }
                | Event::FullscreenConfigure { width, height, .. } => {
                    if *width == 0 || *height == 0 {
                        continue;
                    }
                    window.resize(*width, *height).map_err(os_err)?;
                    size = window.size();
                    screen.width = size.width as u32;
                    screen.height = size.height as u32;
                    wallpaper = background(painter, screen.width, screen.height)?;
                    focus = pipeline.focused_text_input_state().is_some();
                    rebuild = true;
                    break;
                }
                Event::InputPanelOcclusionChanged { occlusion } => {
                    let covered =
                        occlusion.map_or(0., |r| (size.height - r.origin.y).clamp(0., size.height));
                    if screen.covered != covered {
                        screen.covered = covered;
                        focus = pipeline.focused_text_input_state().is_some();
                        rebuild = true;
                        break;
                    }
                    None
                }
                Event::Keyboard(key) => greeter_ui::broker_key(key),
                // TextField retains only the broker's masked display state.
                Event::Input(_)
                | Event::TextInputCommit { .. }
                | Event::TextInputPreedit { .. }
                | Event::TextInputDeleteSurroundingText { .. } => None,
                _ => {
                    pipeline.handle_event(&event);
                    actions.take().map(|a| a.broker_byte())
                }
            };
            if let Some(byte) = key {
                broker.exchange(&[byte], &mut reply)?;
                if reply[0] == 2 {
                    return Ok(());
                }
                let next: [u8; 36] = reply[1..].try_into().unwrap();
                rebuild =
                    next[..2] != screen.display[..2] || (next[0] >= 3 && next != screen.display);
                screen.display = next;
                value.set(screen.value());
                pipeline.request_redraw();
                if rebuild {
                    focus = true;
                    break;
                }
            }
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
    let mut compositor = SeatChild::spawn(
        Command::new(&config.compositor[0])
            .args(&config.compositor[1..])
            .stdin(Stdio::null()),
    )?;
    let result = (|| {
        let mut state = [0; 37];
        let start = Instant::now();
        while !sws_client::Connection::connect(&config.display_socket)
            .is_ok_and(|conn| conn.get_screen_size().is_ok())
        {
            if compositor.exited()? || start.elapsed() >= Duration::from_secs(30) {
                return Err(io::Error::other("compositor did not become ready"));
            }
            thread::sleep(Duration::from_millis(50));
        }
        broker.exchange(&[16], &mut state)?;
        while state[0] == 1 && !compositor.exited()? {
            thread::sleep(Duration::from_millis(200));
            broker.exchange(&[0], &mut state)?;
        }
        Ok(())
    })();
    result.and(compositor.stop())
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
