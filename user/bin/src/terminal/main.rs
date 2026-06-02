//! Terminal - GUI terminal emulator backed by a PTY.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

mod vt;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::f32;
use core::time::Duration;

use scarlet_ui::{
    Application, Color, ComponentElement, Element, KeyCode, KeyEvent, PlatformWindow,
    SWSPlatformWindow, Size, State, StateId, TextGrid, TextGridBuffer, TextGridCell,
    TextGridCursor, View, ViewExt, Window, text_grid_cell_width,
};
use std::fs::File;
use std::handle::Handle;
use std::io::Read;
use std::ipc::{ProcessControl, send_process_control};
use std::pty::{PtyMaster, PtyPair, PtySlave};
use std::sync::Mutex;
use std::task::{
    EXECVE_FORCE_ABI_REBUILD, WAIT_NOHANG, create_session, execve_with_flags, exit, fork,
    process_group_id, waitpid,
};
use std::{format, println, thread};
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
const TERM_ENV: &str = "TERM=xterm-256color";

#[derive(Clone, Copy, Debug)]
struct TerminalTextInput {
    window_id: u32,
    context_id: u32,
    serial: u32,
}

#[derive(Clone, Debug)]
struct TerminalPreedit {
    text: String,
    cursor_byte: u32,
}

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
    text_input: Arc<Mutex<Option<TerminalTextInput>>>,
    preedit: Arc<Mutex<Option<TerminalPreedit>>>,
    child_task: Arc<OwnedChildTask>,
}

impl TerminalApp {
    fn new() -> Self {
        let foreground = Color::rgb(230, 232, 235);
        let background = Color::rgb(12, 14, 18);
        let metrics = TerminalMetrics::default();
        let (columns, rows) = grid_dimensions(DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT, metrics);
        let screen = Arc::new(Mutex::new(VtScreen::new(columns, rows)));
        let mut grid = screen.lock().view_grid();
        grid.write_text(0, 0, "Starting Terminal...", foreground, background);
        Self {
            grid: State::new(StateId::new(0), grid),
            cursor: State::new(StateId::new(1), TextGridCursor::new(0, 1)),
            metrics: State::new(StateId::new(2), metrics),
            screen,
            master_writer: Arc::new(Mutex::new(None)),
            window_size: Arc::new(Mutex::new((DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT))),
            text_input: Arc::new(Mutex::new(None)),
            preedit: Arc::new(Mutex::new(None)),
            child_task: Arc::new(OwnedChildTask::new()),
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
                println!("[terminal] PtyPair::open failed: {}", error);
                self.set_status("PTY open failed");
                return;
            }
        };

        let metrics = self.metrics.get();
        let (width, height) = *self.window_size.lock();
        let (columns, rows) = grid_dimensions(width, height, metrics);
        if let Err(error) = master.set_winsize(columns as u16, rows as u16) {
            println!("[terminal] set_winsize failed: {}", error);
        }

        let master_handle = master.as_file().as_raw();
        let writer = match master.as_file().clone_handle().and_then(File::from_handle) {
            Ok(file) => file,
            Err(error) => {
                println!("[terminal] failed to duplicate PTY master: {:?}", error);
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
        self.child_task.set(shell_pid as u32);
        println!(
            "[terminal] opened {} and started shell pid={}",
            slave_path, shell_pid
        );

        start_reader_thread(
            master,
            self.screen.clone(),
            self.grid.clone(),
            self.cursor.clone(),
            self.preedit.clone(),
        );
    }

    fn finish_child_task(&self) {
        self.child_task.finish();
    }

    fn refresh_text_input_state(&self, window: &mut SWSPlatformWindow) {
        let mut text_input = self.text_input.lock();
        let Some(state) = text_input.as_mut() else {
            return;
        };
        let cursor = self.cursor.get();
        let metrics = self.metrics.get();
        let x = (cursor.column as f32 * metrics.cell_width) as i32;
        let y = (cursor.row as f32 * metrics.cell_height) as i32;
        let width = metrics.cell_width.max(1.0) as u32;
        let height = metrics.cell_height.max(1.0) as u32;

        let conn = window.connection_mut();
        let _ = conn.set_text_input_cursor_rect(state.context_id, x, y, width, height);
        let _ = conn.set_text_input_surrounding_text(state.context_id, 0, 0, "");
        let _ = conn.set_text_input_content_type(
            state.context_id,
            sws_protocol::text_input_content_hints::MULTILINE,
            sws_protocol::text_input_content_purpose::TERMINAL,
        );
        if conn
            .commit_text_input_state(state.context_id, state.serial)
            .is_ok()
        {
            state.serial = state.serial.saturating_add(1);
        }
    }
}

struct OwnedChildTask {
    pid: Mutex<Option<u32>>,
}

impl OwnedChildTask {
    fn new() -> Self {
        Self {
            pid: Mutex::new(None),
        }
    }

    fn set(&self, pid: u32) {
        *self.pid.lock() = Some(pid);
    }

    fn finish(&self) {
        let Some(pid) = self.pid.lock().take() else {
            return;
        };

        if reap_child_task(pid, 8) {
            return;
        }

        let _ = send_process_control(pid, ProcessControl::Terminate);
        if reap_child_task(pid, 16) {
            return;
        }

        let _ = send_process_control(pid, ProcessControl::Kill);
        let _ = reap_child_task(pid, 8);
    }
}

impl Drop for OwnedChildTask {
    fn drop(&mut self) {
        self.finish();
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
        let preedit = self.preedit.clone();
        let metrics = self.metrics.get();
        Window::new(
            "Terminal",
            TextGrid::new(self.grid.clone())
                .cell_size(metrics.cell_width, metrics.cell_height)
                .font_size(metrics.font_size)
                .background_color(Color::rgb(12, 14, 18))
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
                        &preedit,
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

    fn on_window_created(&mut self, window: &mut SWSPlatformWindow) {
        let window_id = window.surface_id();
        let Ok((context_id, serial)) = window
            .connection_mut()
            .create_text_input_context(window_id, 0)
        else {
            println!("[terminal] failed to create text-input context");
            return;
        };

        *self.text_input.lock() = Some(TerminalTextInput {
            window_id,
            context_id,
            serial,
        });

        self.refresh_text_input_state(window);

        if let Err(error) = window.connection_mut().enable_text_input(context_id) {
            println!("[terminal] failed to enable text-input: {:?}", error);
        } else {
            println!(
                "[terminal] text-input enabled context={} window={}",
                context_id, window_id
            );
        }
    }

    fn on_focus_changed(&mut self, window_id: u32, _app_name: &str, _menu_titles: &str) {
        let Some(text_input) = *self.text_input.lock() else {
            return;
        };
        if text_input.window_id != window_id {
            return;
        }
        println!(
            "[terminal] focused text-input context={}",
            text_input.context_id
        );
    }

    fn on_text_input_commit(&mut self, context_id: u32, _serial: u32, text: &str) {
        let Some(text_input) = *self.text_input.lock() else {
            return;
        };
        if text_input.context_id != context_id {
            return;
        }
        self.clear_preedit();
        println!("[terminal] text-input commit: {}", text);
        write_bytes(&self.master_writer, text.as_bytes());
    }

    fn on_text_input_preedit(
        &mut self,
        context_id: u32,
        _serial: u32,
        cursor_byte: u32,
        text: &str,
    ) {
        let Some(text_input) = *self.text_input.lock() else {
            return;
        };
        if text_input.context_id != context_id {
            return;
        }
        if text.is_empty() {
            self.clear_preedit();
        } else {
            *self.preedit.lock() = Some(TerminalPreedit {
                text: String::from(text),
                cursor_byte,
            });
            refresh_terminal_view(&self.screen, &self.grid, &self.cursor, &self.preedit);
        }
    }

    fn on_text_input_delete_surrounding_text(
        &mut self,
        context_id: u32,
        _serial: u32,
        _before_bytes: u32,
        _after_bytes: u32,
    ) {
        let Some(text_input) = *self.text_input.lock() else {
            return;
        };
        if text_input.context_id == context_id {
            self.clear_preedit();
        }
    }

    fn on_resize(&mut self, width: u32, height: u32) {
        *self.window_size.lock() = (width, height);
        resize_terminal(
            &self.screen,
            &self.grid,
            &self.cursor,
            &self.master_writer,
            &self.preedit,
            width,
            height,
            self.metrics.get(),
        );
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

impl TerminalApp {
    fn clear_preedit(&self) {
        if self.preedit.lock().take().is_some() {
            refresh_terminal_view(&self.screen, &self.grid, &self.cursor, &self.preedit);
        }
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
            let env_strings = shell_environment();
            let env_refs: Vec<&str> = env_strings.iter().map(|s| s.as_str()).collect();
            for path in candidates {
                let argv = [path];
                let rc = execve_with_flags(path, &argv, &env_refs, EXECVE_FORCE_ABI_REBUILD);
                if rc == 0 {
                    break;
                }
            }
            println!("[terminal] failed to exec shell");
            exit(127);
        }
        pid => pid,
    }
}

fn shell_environment() -> Vec<String> {
    let mut env = Vec::new();
    let mut has_term = false;

    for (key, value) in std::env::vars() {
        if key == "TERM" {
            has_term = true;
            env.push(String::from(TERM_ENV));
        } else {
            env.push(format!("{key}={value}"));
        }
    }

    if !has_term {
        env.push(String::from(TERM_ENV));
    }

    env
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
                    "[terminal] warning: duplicated handle {}, expected {}",
                    handle.as_raw(),
                    raw_handle
                );
            }
            core::mem::forget(handle);
        }
        Err(error) => {
            println!(
                "[terminal] failed to duplicate stdio handle {}: {:?}",
                raw_handle, error
            );
        }
    }
}

fn reap_child_task(pid: u32, attempts: usize) -> bool {
    for _ in 0..attempts {
        let (changed_pid, _) = waitpid(pid as i32, WAIT_NOHANG);
        if changed_pid == pid as i32 || changed_pid < 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

fn start_reader_thread(
    mut master: PtyMaster,
    screen: Arc<Mutex<VtScreen>>,
    grid: State<TextGridBuffer>,
    cursor: State<TextGridCursor>,
    preedit: Arc<Mutex<Option<TerminalPreedit>>>,
) {
    thread::spawn(move || {
        {
            refresh_terminal_view(&screen, &grid, &cursor, &preedit);
        }

        let mut buffer = [0u8; 512];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => {
                    println!("[terminal] PTY closed; exiting");
                    exit(0);
                }
                Ok(count) => {
                    let mut screen = screen.lock();
                    screen.feed(&buffer[..count]);
                    publish_terminal_view(&screen, &grid, &cursor, &preedit);
                }
                Err(error) => {
                    println!("[terminal] PTY read failed: {}", error);
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
    preedit: &Arc<Mutex<Option<TerminalPreedit>>>,
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
                KeyCode::PageUp => scroll_view(screen, grid, cursor, preedit, 10),
                KeyCode::PageDown => scroll_view(screen, grid, cursor, preedit, -10),
                KeyCode::Insert => write_bytes(writer, b"\x1b[2~"),
                KeyCode::Delete => write_bytes(writer, b"\x1b[3~"),
                KeyCode::F(1) => write_bytes(writer, b"\x1bOP"),
                KeyCode::F(2) => write_bytes(writer, b"\x1bOQ"),
                KeyCode::F(3) => write_bytes(writer, b"\x1bOR"),
                KeyCode::F(4) => write_bytes(writer, b"\x1bOS"),
                KeyCode::F(11) => adjust_font_size(
                    metrics,
                    screen,
                    grid,
                    cursor,
                    writer,
                    window_size,
                    preedit,
                    -1.0,
                ),
                KeyCode::F(12) => adjust_font_size(
                    metrics,
                    screen,
                    grid,
                    cursor,
                    writer,
                    window_size,
                    preedit,
                    1.0,
                ),
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
    preedit: &Arc<Mutex<Option<TerminalPreedit>>>,
    lines: isize,
) {
    let mut screen = screen.lock();
    screen.scroll_view(lines);
    publish_terminal_view(&screen, grid, cursor, preedit);
}

fn adjust_font_size(
    metrics: &State<TerminalMetrics>,
    screen: &Arc<Mutex<VtScreen>>,
    grid: &State<TextGridBuffer>,
    cursor: &State<TextGridCursor>,
    writer: &Arc<Mutex<Option<File>>>,
    window_size: &Arc<Mutex<(u32, u32)>>,
    preedit: &Arc<Mutex<Option<TerminalPreedit>>>,
    delta: f32,
) {
    let next = metrics.get().with_font_delta(delta);
    if next == metrics.get() {
        return;
    }
    metrics.set(next);
    let (width, height) = *window_size.lock();
    resize_terminal(screen, grid, cursor, writer, preedit, width, height, next);
}

fn resize_terminal(
    screen: &Arc<Mutex<VtScreen>>,
    grid: &State<TextGridBuffer>,
    cursor: &State<TextGridCursor>,
    writer: &Arc<Mutex<Option<File>>>,
    preedit: &Arc<Mutex<Option<TerminalPreedit>>>,
    width: u32,
    height: u32,
    metrics: TerminalMetrics,
) {
    let (columns, rows) = grid_dimensions(width, height, metrics);
    {
        let mut screen = screen.lock();
        screen.resize(columns, rows);
        publish_terminal_view(&screen, grid, cursor, preedit);
    }
    let mut guard = writer.lock();
    if let Some(master) = guard.as_mut() {
        let _ = std::tty::Terminal::from_file(master)
            .set_winsize(std::tty::WindowSize::new(columns as u16, rows as u16));
    }
}

fn refresh_terminal_view(
    screen: &Arc<Mutex<VtScreen>>,
    grid: &State<TextGridBuffer>,
    cursor: &State<TextGridCursor>,
    preedit: &Arc<Mutex<Option<TerminalPreedit>>>,
) {
    let screen = screen.lock();
    publish_terminal_view(&screen, grid, cursor, preedit);
}

fn publish_terminal_view(
    screen: &VtScreen,
    grid: &State<TextGridBuffer>,
    cursor: &State<TextGridCursor>,
    preedit: &Arc<Mutex<Option<TerminalPreedit>>>,
) {
    let base_cursor = screen.cursor();
    let mut view = screen.view_grid();
    let mut display_cursor = base_cursor;

    if let Some(preedit) = preedit.lock().clone() {
        display_cursor = draw_preedit(&mut view, base_cursor, &preedit);
    }

    grid.set(view);
    cursor.set(display_cursor);
}

fn draw_preedit(
    grid: &mut TextGridBuffer,
    base_cursor: TextGridCursor,
    preedit: &TerminalPreedit,
) -> TextGridCursor {
    let foreground = Color::rgb(245, 248, 255);
    let background = Color::rgb(44, 55, 70);
    let mut column = base_cursor.column;
    let mut row = base_cursor.row;
    let cursor_offset = preedit_cursor_offset(&preedit.text, preedit.cursor_byte);
    let mut display_cursor = base_cursor;

    for (offset, ch) in preedit.text.chars().enumerate() {
        if row >= grid.rows() {
            break;
        }
        if offset == cursor_offset {
            display_cursor = TextGridCursor {
                column,
                row,
                visible: true,
            };
        }
        let width = text_grid_cell_width(ch);
        if width == 2 && column + 1 >= grid.columns() {
            column = 0;
            row += 1;
            if row >= grid.rows() {
                break;
            }
        }

        let mut cell = TextGridCell::new(ch, foreground, background);
        cell.underline = true;
        let _ = grid.set_cell(column, row, cell);
        if width == 2 {
            let mut continuation = TextGridCell::new('\0', foreground, background);
            continuation.underline = true;
            let _ = grid.set_cell(column + 1, row, continuation);
        }

        column += width;
        if column >= grid.columns() {
            column = 0;
            row += 1;
        }
    }

    if cursor_offset >= preedit.text.chars().count() {
        display_cursor = TextGridCursor {
            column: column.min(grid.columns().saturating_sub(1)),
            row: row.min(grid.rows().saturating_sub(1)),
            visible: true,
        };
    }

    display_cursor
}

fn preedit_cursor_offset(text: &str, cursor_byte: u32) -> usize {
    let cursor_byte = cursor_byte as usize;
    text.char_indices()
        .take_while(|(byte_offset, _)| *byte_offset < cursor_byte)
        .count()
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
        println!("[terminal] pty writer is not ready");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[terminal] Starting");
    let mut app = TerminalApp::new();
    let result = app.run();
    app.finish_child_task();
    match result {
        Ok(()) => 0,
        Err(error) => {
            println!("[terminal] Application error: {}", error);
            1
        }
    }
}
