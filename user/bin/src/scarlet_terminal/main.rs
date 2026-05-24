//! Scarlet Terminal - GUI terminal emulator backed by a PTY.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

mod vt;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use core::any::Any;
use core::f32;

use scarlet_ui::{
    Application, Color, ComponentElement, Element, KeyCode, KeyEvent, Size, State, StateId,
    TextGrid, TextGridBuffer, TextGridCell, TextGridCursor, View, ViewExt, Window,
};
use std::fs::File;
use std::handle::Handle;
use std::io::Read;
use std::pty::{PtyMaster, PtyPair, PtySlave};
use std::sync::Mutex;
use std::task::{
    EXECVE_FORCE_ABI_REBUILD, create_session, execve_with_flags, exit, fork, process_group_id,
};
use std::{println, thread};
use vt::VtScreen;

const DEFAULT_WINDOW_WIDTH: u32 = 900;
const DEFAULT_WINDOW_HEIGHT: u32 = 620;
const DEFAULT_CELL_WIDTH: f32 = 9.0;
const DEFAULT_CELL_HEIGHT: f32 = 18.0;
const DEFAULT_FONT_SIZE: f32 = 16.0;
const MIN_FONT_SIZE: f32 = 10.0;
const MAX_FONT_SIZE: f32 = 28.0;
const WINDOW_HORIZONTAL_DECORATION: f32 = 4.0;
const WINDOW_VERTICAL_DECORATION: f32 = 34.0;
const SHELL_ENV: [&str; 7] = [
    "HOME=/system/scarlet/root",
    "PWD=/",
    "SHELL=/system/scarlet/bin/sh",
    "TERM=xterm-256color",
    "PATH=/system/scarlet/bin:/bin:/scarlet/system/scarlet/bin:/scarlet/system/linux-aarch64/bin:/scarlet/system/linux-aarch64/usr/bin:/old_root/system/scarlet/bin",
    "XDG_RUNTIME_DIR=/tmp",
    "WAYLAND_DISPLAY=wayland-0",
];

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalMetrics {
    cell_width: f32,
    cell_height: f32,
    font_size: f32,
}

impl TerminalMetrics {
    fn default() -> Self {
        Self {
            cell_width: DEFAULT_CELL_WIDTH,
            cell_height: DEFAULT_CELL_HEIGHT,
            font_size: DEFAULT_FONT_SIZE,
        }
    }

    fn with_font_delta(self, delta: f32) -> Self {
        let font_size = (self.font_size + delta).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        let scale = font_size / DEFAULT_FONT_SIZE;
        Self {
            cell_width: (DEFAULT_CELL_WIDTH * scale).max(1.0),
            cell_height: (DEFAULT_CELL_HEIGHT * scale).max(1.0),
            font_size,
        }
    }
}

#[derive(Clone)]
struct TerminalApp {
    grid: State<TextGridBuffer>,
    cursor: State<TextGridCursor>,
    metrics: State<TerminalMetrics>,
    screen: Arc<Mutex<VtScreen>>,
    master_writer: Arc<Mutex<Option<File>>>,
    window_size: Arc<Mutex<(u32, u32)>>,
}

impl TerminalApp {
    fn new() -> Self {
        let foreground = Color::rgb(230, 232, 235);
        let background = Color::rgb(12, 14, 18);
        let metrics = TerminalMetrics::default();
        let (columns, rows) = grid_dimensions(DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT, metrics);
        let screen = Arc::new(Mutex::new(VtScreen::new(columns, rows)));
        let mut grid = screen.lock().view_grid();
        grid.write_text(0, 0, "Starting Scarlet Terminal...", foreground, background);
        Self {
            grid: State::new(StateId::new(0), grid),
            cursor: State::new(StateId::new(1), TextGridCursor::new(0, 1)),
            metrics: State::new(StateId::new(2), metrics),
            screen,
            master_writer: Arc::new(Mutex::new(None)),
            window_size: Arc::new(Mutex::new((DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT))),
        }
    }

    fn set_status(&self, message: &str) {
        let foreground = Color::rgb(230, 232, 235);
        let background = Color::rgb(12, 14, 18);
        self.grid.update(|grid| {
            grid.clear(TextGridCell::blank(foreground, background));
            grid.write_text(0, 0, message, foreground, background);
        });
        self.cursor.set(TextGridCursor::new(0, 1));
    }

    fn start_pty_session(&self) {
        let PtyPair {
            master,
            slave,
            slave_path,
        } = match PtyPair::open() {
            Ok(pair) => pair,
            Err(error) => {
                println!("[scarlet_terminal] PtyPair::open failed: {}", error);
                self.set_status("PTY open failed");
                return;
            }
        };

        let metrics = self.metrics.get();
        let (width, height) = *self.window_size.lock();
        let (columns, rows) = grid_dimensions(width, height, metrics);
        if let Err(error) = master.set_winsize(columns as u16, rows as u16) {
            println!("[scarlet_terminal] set_winsize failed: {}", error);
        }

        let master_handle = master.as_file().as_raw();
        let writer = match master.as_file().clone_handle().and_then(File::from_handle) {
            Ok(file) => file,
            Err(error) => {
                println!(
                    "[scarlet_terminal] failed to duplicate PTY master: {:?}",
                    error
                );
                self.set_status("PTY duplicate failed");
                return;
            }
        };
        let writer_handle = writer.as_raw();
        *self.master_writer.lock() = Some(writer);

        let shell_pid = spawn_shell(slave, master_handle, writer_handle);
        if shell_pid < 0 {
            self.set_status("Failed to start shell");
            return;
        }
        println!(
            "[scarlet_terminal] opened {} and started shell pid={}",
            slave_path, shell_pid
        );

        start_reader_thread(
            master,
            self.screen.clone(),
            self.grid.clone(),
            self.cursor.clone(),
        );
    }
}

impl View for TerminalApp {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new(self.clone()))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn scarlet_ui::Listenable> {
        vec![&self.grid, &self.cursor, &self.metrics]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Application for TerminalApp {
    fn body(&self) -> impl View {
        let writer = self.master_writer.clone();
        let screen = self.screen.clone();
        let grid = self.grid.clone();
        let cursor = self.cursor.clone();
        let metrics_state = self.metrics.clone();
        let window_size = self.window_size.clone();
        let metrics = self.metrics.get();
        Window::new(
            "Scarlet Terminal",
            TextGrid::new(self.grid.clone())
                .cell_size(metrics.cell_width, metrics.cell_height)
                .font_size(metrics.font_size)
                .cursor(Some(self.cursor.get()))
                .cursor_color(Color::rgba_f32(0.85, 0.92, 1.0, 0.8))
                .on_key(move |event| {
                    write_key_event(
                        &writer,
                        &screen,
                        &grid,
                        &cursor,
                        &metrics_state,
                        &window_size,
                        event,
                    )
                })
                .frame(f32::INFINITY, f32::INFINITY),
        )
        .app_id("org.scarlet-os.desktop.terminal")
        .size(Size::new(
            DEFAULT_WINDOW_WIDTH as f32,
            DEFAULT_WINDOW_HEIGHT as f32,
        ))
    }

    fn init(&mut self) {
        self.start_pty_session();
    }

    fn on_resize(&mut self, width: u32, height: u32) {
        *self.window_size.lock() = (width, height);
        resize_terminal(
            &self.screen,
            &self.grid,
            &self.cursor,
            &self.master_writer,
            width,
            height,
            self.metrics.get(),
        );
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn spawn_shell(slave: PtySlave, inherited_master_handle: i32, inherited_writer_handle: i32) -> i32 {
    match fork() {
        0 => {
            close_inherited_handle(inherited_master_handle);
            close_inherited_handle(inherited_writer_handle);
            let _ = create_session();
            setup_child_stdio(slave);
            let candidates = [
                "/bin/sh",
                "/scarlet/system/scarlet/bin/sh",
                "/old_root/bin/sh",
            ];
            for path in candidates {
                let argv = [path];
                let rc = execve_with_flags(path, &argv, &SHELL_ENV, EXECVE_FORCE_ABI_REBUILD);
                if rc == 0 {
                    break;
                }
            }
            println!("[scarlet_terminal] failed to exec shell");
            exit(127);
        }
        pid => pid,
    }
}

fn close_inherited_handle(raw_handle: i32) {
    if raw_handle >= 0
        && let Ok(handle) = unsafe { Handle::from_raw(raw_handle) }
    {
        drop(handle);
    }
}

fn setup_child_stdio(slave: PtySlave) {
    let slave_handle = slave.into_file().into_handle();
    let terminal = std::tty::Terminal::from_handle(&slave_handle);
    let _ = terminal.acquire_as_controlling(false);
    if let Ok(pgid) = process_group_id(None) {
        let _ = terminal.set_foreground_group(pgid as usize);
    }
    duplicate_to_stdio(&slave_handle, 0);
    duplicate_to_stdio(&slave_handle, 1);
    duplicate_to_stdio(&slave_handle, 2);
}

fn duplicate_to_stdio(source: &Handle, raw_handle: i32) {
    if let Ok(handle) = unsafe { Handle::from_raw(raw_handle) } {
        let _ = handle.close();
    }

    match source.duplicate() {
        Ok(handle) => {
            if handle.as_raw() != raw_handle {
                println!(
                    "[scarlet_terminal] warning: duplicated handle {}, expected {}",
                    handle.as_raw(),
                    raw_handle
                );
            }
            core::mem::forget(handle);
        }
        Err(error) => {
            println!(
                "[scarlet_terminal] failed to duplicate stdio handle {}: {:?}",
                raw_handle, error
            );
        }
    }
}

fn start_reader_thread(
    mut master: PtyMaster,
    screen: Arc<Mutex<VtScreen>>,
    grid: State<TextGridBuffer>,
    cursor: State<TextGridCursor>,
) {
    thread::spawn(move || {
        {
            let screen = screen.lock();
            grid.set(screen.view_grid());
            cursor.set(screen.cursor());
        }

        let mut buffer = [0u8; 512];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => {
                    println!("[scarlet_terminal] PTY closed; exiting");
                    exit(0);
                }
                Ok(count) => {
                    let mut screen = screen.lock();
                    screen.feed(&buffer[..count]);
                    grid.set(screen.view_grid());
                    cursor.set(screen.cursor());
                }
                Err(error) => {
                    println!("[scarlet_terminal] PTY read failed: {}", error);
                    exit(1);
                }
            }
        }
    });
}

fn write_key_event(
    writer: &Arc<Mutex<Option<File>>>,
    screen: &Arc<Mutex<VtScreen>>,
    grid: &State<TextGridBuffer>,
    cursor: &State<TextGridCursor>,
    metrics: &State<TerminalMetrics>,
    window_size: &Arc<Mutex<(u32, u32)>>,
    event: KeyEvent,
) -> bool {
    match event {
        KeyEvent::Char { c } => {
            let mut encoded = [0u8; 4];
            write_bytes(writer, c.encode_utf8(&mut encoded).as_bytes());
            true
        }
        KeyEvent::Pressed { keycode } => {
            match keycode {
                KeyCode::Char(c) if c.is_control() => {
                    let mut encoded = [0u8; 4];
                    write_bytes(writer, c.encode_utf8(&mut encoded).as_bytes());
                }
                KeyCode::Enter => write_bytes(writer, b"\r"),
                KeyCode::Tab => write_bytes(writer, b"\t"),
                KeyCode::Backspace => write_bytes(writer, &[0x7f]),
                KeyCode::Escape => write_bytes(writer, &[0x1b]),
                KeyCode::Left => write_bytes(writer, b"\x1b[D"),
                KeyCode::Right => write_bytes(writer, b"\x1b[C"),
                KeyCode::Up => write_bytes(writer, b"\x1b[A"),
                KeyCode::Down => write_bytes(writer, b"\x1b[B"),
                KeyCode::Home => write_bytes(writer, b"\x1b[H"),
                KeyCode::End => write_bytes(writer, b"\x1b[F"),
                KeyCode::PageUp => scroll_view(screen, grid, cursor, 10),
                KeyCode::PageDown => scroll_view(screen, grid, cursor, -10),
                KeyCode::Insert => write_bytes(writer, b"\x1b[2~"),
                KeyCode::Delete => write_bytes(writer, b"\x1b[3~"),
                KeyCode::F(1) => write_bytes(writer, b"\x1bOP"),
                KeyCode::F(2) => write_bytes(writer, b"\x1bOQ"),
                KeyCode::F(3) => write_bytes(writer, b"\x1bOR"),
                KeyCode::F(4) => write_bytes(writer, b"\x1bOS"),
                KeyCode::F(11) => {
                    adjust_font_size(metrics, screen, grid, cursor, writer, window_size, -1.0)
                }
                KeyCode::F(12) => {
                    adjust_font_size(metrics, screen, grid, cursor, writer, window_size, 1.0)
                }
                _ => {}
            }
            true
        }
        KeyEvent::Released { .. } => true,
    }
}

fn scroll_view(
    screen: &Arc<Mutex<VtScreen>>,
    grid: &State<TextGridBuffer>,
    cursor: &State<TextGridCursor>,
    lines: isize,
) {
    let mut screen = screen.lock();
    screen.scroll_view(lines);
    grid.set(screen.view_grid());
    cursor.set(screen.cursor());
}

fn adjust_font_size(
    metrics: &State<TerminalMetrics>,
    screen: &Arc<Mutex<VtScreen>>,
    grid: &State<TextGridBuffer>,
    cursor: &State<TextGridCursor>,
    writer: &Arc<Mutex<Option<File>>>,
    window_size: &Arc<Mutex<(u32, u32)>>,
    delta: f32,
) {
    let next = metrics.get().with_font_delta(delta);
    if next == metrics.get() {
        return;
    }
    metrics.set(next);
    let (width, height) = *window_size.lock();
    resize_terminal(screen, grid, cursor, writer, width, height, next);
}

fn resize_terminal(
    screen: &Arc<Mutex<VtScreen>>,
    grid: &State<TextGridBuffer>,
    cursor: &State<TextGridCursor>,
    writer: &Arc<Mutex<Option<File>>>,
    width: u32,
    height: u32,
    metrics: TerminalMetrics,
) {
    let (columns, rows) = grid_dimensions(width, height, metrics);
    {
        let mut screen = screen.lock();
        screen.resize(columns, rows);
        grid.set(screen.view_grid());
        cursor.set(screen.cursor());
    }
    let mut guard = writer.lock();
    if let Some(master) = guard.as_mut() {
        let _ = std::tty::Terminal::from_file(master)
            .set_winsize(std::tty::WindowSize::new(columns as u16, rows as u16));
    }
}

fn grid_dimensions(width: u32, height: u32, metrics: TerminalMetrics) -> (usize, usize) {
    let content_width = (width as f32 - WINDOW_HORIZONTAL_DECORATION).max(metrics.cell_width);
    let content_height = (height as f32 - WINDOW_VERTICAL_DECORATION).max(metrics.cell_height);
    (
        (content_width / metrics.cell_width).max(1.0) as usize,
        (content_height / metrics.cell_height).max(1.0) as usize,
    )
}

fn write_bytes(writer: &Arc<Mutex<Option<File>>>, bytes: &[u8]) {
    let mut guard = writer.lock();
    if let Some(file) = guard.as_mut() {
        let _ = file.write_all(bytes);
    } else {
        println!("[scarlet_terminal] pty writer is not ready");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[scarlet_terminal] Starting");
    let mut app = TerminalApp::new();
    match app.run() {
        Ok(()) => 0,
        Err(error) => {
            println!("[scarlet_terminal] Application error: {}", error);
            1
        }
    }
}
