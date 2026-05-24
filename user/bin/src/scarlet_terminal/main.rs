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

const COLUMNS: usize = 100;
const ROWS: usize = 32;
const CELL_WIDTH: f32 = 9.0;
const CELL_HEIGHT: f32 = 18.0;
const FONT_SIZE: f32 = 16.0;

#[derive(Clone)]
struct TerminalApp {
    grid: State<TextGridBuffer>,
    cursor: State<TextGridCursor>,
    master_writer: Arc<Mutex<Option<File>>>,
}

impl TerminalApp {
    fn new() -> Self {
        let foreground = Color::rgb(230, 232, 235);
        let background = Color::rgb(12, 14, 18);
        let mut grid =
            TextGridBuffer::new(COLUMNS, ROWS, TextGridCell::blank(foreground, background));
        grid.write_text(0, 0, "Starting Scarlet Terminal...", foreground, background);
        Self {
            grid: State::new(StateId::new(0), grid),
            cursor: State::new(StateId::new(1), TextGridCursor::new(0, 1)),
            master_writer: Arc::new(Mutex::new(None)),
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

        if let Err(error) = master.set_winsize(COLUMNS as u16, ROWS as u16) {
            println!("[scarlet_terminal] set_winsize failed: {}", error);
        }

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
        *self.master_writer.lock() = Some(writer);

        let shell_pid = spawn_shell(slave);
        if shell_pid < 0 {
            self.set_status("Failed to start shell");
            return;
        }
        println!(
            "[scarlet_terminal] opened {} and started shell pid={}",
            slave_path, shell_pid
        );

        start_reader_thread(master, self.grid.clone(), self.cursor.clone());
    }
}

impl View for TerminalApp {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new(self.clone()))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn scarlet_ui::Listenable> {
        vec![&self.grid, &self.cursor]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Application for TerminalApp {
    fn body(&self) -> impl View {
        let writer = self.master_writer.clone();
        Window::new(
            "Scarlet Terminal",
            TextGrid::new(self.grid.clone())
                .cell_size(CELL_WIDTH, CELL_HEIGHT)
                .font_size(FONT_SIZE)
                .cursor(Some(self.cursor.get()))
                .cursor_color(Color::rgba_f32(0.85, 0.92, 1.0, 0.8))
                .on_key(move |event| write_key_event(&writer, event))
                .frame(f32::INFINITY, f32::INFINITY),
        )
        .app_id("org.scarlet-os.desktop.terminal")
        .size(Size::new(900.0, 620.0))
    }

    fn init(&mut self) {
        self.start_pty_session();
    }

    fn on_resize(&mut self, width: u32, height: u32) {
        let columns = (width as f32 / CELL_WIDTH).max(1.0) as u16;
        let rows = (height as f32 / CELL_HEIGHT).max(1.0) as u16;
        let mut guard = self.master_writer.lock();
        if let Some(master) = guard.as_mut() {
            let _ = std::tty::Terminal::from_file(master)
                .set_winsize(std::tty::WindowSize::new(columns, rows));
        }
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn spawn_shell(slave: PtySlave) -> i32 {
    match fork() {
        0 => {
            let _ = create_session();
            setup_child_stdio(slave);
            let candidates = [
                "/bin/sh",
                "/scarlet/system/scarlet/bin/sh",
                "/old_root/bin/sh",
            ];
            for path in candidates {
                let argv = [path];
                let rc = execve_with_flags(path, &argv, &[], EXECVE_FORCE_ABI_REBUILD);
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

fn duplicate_to_stdio(source: &Handle, raw_fd: i32) {
    if let Ok(handle) = unsafe { Handle::from_raw(raw_fd) } {
        let _ = handle.close();
    }

    match source.duplicate() {
        Ok(handle) => {
            if handle.as_raw() != raw_fd {
                println!(
                    "[scarlet_terminal] warning: duplicated fd {}, expected {}",
                    handle.as_raw(),
                    raw_fd
                );
            }
            core::mem::forget(handle);
        }
        Err(error) => {
            println!(
                "[scarlet_terminal] failed to duplicate stdio fd {}: {:?}",
                raw_fd, error
            );
        }
    }
}

fn start_reader_thread(
    mut master: PtyMaster,
    grid: State<TextGridBuffer>,
    cursor: State<TextGridCursor>,
) {
    thread::spawn(move || {
        let mut screen = VtScreen::new(COLUMNS, ROWS);
        grid.set(screen.grid().clone());
        cursor.set(screen.cursor());

        let mut buffer = [0u8; 512];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    screen.feed(&buffer[..count]);
                    grid.set(screen.grid().clone());
                    cursor.set(screen.cursor());
                }
                Err(error) => {
                    println!("[scarlet_terminal] PTY read failed: {}", error);
                    break;
                }
            }
        }
    });
}

fn write_key_event(writer: &Arc<Mutex<Option<File>>>, event: KeyEvent) -> bool {
    match event {
        KeyEvent::Char { c } => {
            let mut encoded = [0u8; 4];
            write_bytes(writer, c.encode_utf8(&mut encoded).as_bytes());
            true
        }
        KeyEvent::Pressed { keycode } => {
            match keycode {
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
                KeyCode::PageUp => write_bytes(writer, b"\x1b[5~"),
                KeyCode::PageDown => write_bytes(writer, b"\x1b[6~"),
                KeyCode::Insert => write_bytes(writer, b"\x1b[2~"),
                KeyCode::Delete => write_bytes(writer, b"\x1b[3~"),
                KeyCode::F(1) => write_bytes(writer, b"\x1bOP"),
                KeyCode::F(2) => write_bytes(writer, b"\x1bOQ"),
                KeyCode::F(3) => write_bytes(writer, b"\x1bOR"),
                KeyCode::F(4) => write_bytes(writer, b"\x1bOS"),
                _ => {}
            }
            true
        }
        KeyEvent::Released { .. } => true,
    }
}

fn write_bytes(writer: &Arc<Mutex<Option<File>>>, bytes: &[u8]) {
    let mut guard = writer.lock();
    if let Some(file) = guard.as_mut() {
        let _ = file.write_all(bytes);
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
