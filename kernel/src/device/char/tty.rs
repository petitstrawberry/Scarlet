//! TTY (Terminal) device implementation.
//!
//! This module implements a TTY device that acts as a terminal interface
//! providing line discipline, echo, and basic terminal I/O operations.

extern crate alloc;
use crate::arch::Trapframe;
use crate::device::char::{CharDevice, TtyControl};
use crate::device::events::{DeviceEvent, DeviceEventListener, InputEvent};
use crate::device::manager::DeviceManager;
use crate::device::{Device, DeviceCapability, DeviceType};
use crate::late_initcall;
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::sync::waker::Waker;
use crate::task::mytask;
use crate::timer::{TimerHandler, add_timer, cancel_timer, get_tick};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use core::any::Any;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use spin::Mutex;

/// Scarlet-private, OS-agnostic control opcodes for TTY devices.
/// These are stable only within Scarlet and must be mapped by ABI adapters.
pub mod tty_ctl {
    /// Magic 'ST' (0x53, 0x54) followed by sequential IDs to avoid collisions.
    pub const SCTL_TTY_SET_ECHO: u32 = 0x5354_0001;
    pub const SCTL_TTY_GET_ECHO: u32 = 0x5354_0002;
    pub const SCTL_TTY_SET_CANONICAL: u32 = 0x5354_0003;
    pub const SCTL_TTY_GET_CANONICAL: u32 = 0x5354_0004;
    /// arg = (cols<<16 | rows)
    pub const SCTL_TTY_SET_WINSIZE: u32 = 0x5354_0005;
    /// ret = (cols<<16 | rows)
    pub const SCTL_TTY_GET_WINSIZE: u32 = 0x5354_0006;
    /// Set read policy as a neutral abstraction:
    /// arg = ((timeout_ms as u32) << 16) | (min_ready_bytes as u32)
    pub const SCTL_TTY_SET_READ_POLICY: u32 = 0x5354_0007;
    /// Get read policy in the same packed format as SET_READ_POLICY
    pub const SCTL_TTY_GET_READ_POLICY: u32 = 0x5354_0008;
    /// Flush input buffer (arg ignored)
    pub const SCTL_TTY_FLUSH_INPUT: u32 = 0x5354_0009;
    /// Enable/disable debug logging of received bytes (arg!=0 enable)
    pub const SCTL_TTY_SET_DEBUG: u32 = 0x5354_000A;
    /// Get debug logging state (ret=0/1)
    pub const SCTL_TTY_GET_DEBUG: u32 = 0x5354_000B;
    /// Set keyboard mode (0=XLATE, 1=MEDIUMRAW, 2=RAW)
    pub const SCTL_TTY_SET_KBMODE: u32 = 0x5354_000C;
    /// Get keyboard mode (0=XLATE, 1=MEDIUMRAW, 2=RAW)
    pub const SCTL_TTY_GET_KBMODE: u32 = 0x5354_000D;
    /// Set foreground process group ID (arg = namespace-local PGID).
    pub const SCTL_TTY_SET_FOREGROUND_GROUP: u32 = 0x5354_000E;
    /// Get foreground process group ID (ret = namespace-local PGID, or -1 if none).
    pub const SCTL_TTY_GET_FOREGROUND_GROUP: u32 = 0x5354_000F;
    /// Enable/disable terminal-generated process-control events.
    pub const SCTL_TTY_SET_SIGNAL_CHARS: u32 = 0x5354_0010;
    /// Get terminal-generated process-control event state (ret=0/1).
    pub const SCTL_TTY_GET_SIGNAL_CHARS: u32 = 0x5354_0011;
    /// Enable/disable carriage-return to newline input mapping.
    pub const SCTL_TTY_SET_CRNL_INPUT: u32 = 0x5354_0012;
    /// Get carriage-return to newline input mapping state (ret=0/1).
    pub const SCTL_TTY_GET_CRNL_INPUT: u32 = 0x5354_0013;
    /// Enable/disable output post-processing.
    pub const SCTL_TTY_SET_OUTPUT_POSTPROCESS: u32 = 0x5354_0014;
    /// Get output post-processing state (ret=0/1).
    pub const SCTL_TTY_GET_OUTPUT_POSTPROCESS: u32 = 0x5354_0015;
    /// Enable/disable implementation-defined extended line editing.
    pub const SCTL_TTY_SET_EXTENDED_INPUT: u32 = 0x5354_0016;
    /// Get extended line editing state (ret=0/1).
    pub const SCTL_TTY_GET_EXTENDED_INPUT: u32 = 0x5354_0017;
    /// Acquire this terminal as the caller's controlling terminal.
    pub const SCTL_TTY_ACQUIRE_CONTROLLING: u32 = 0x5354_0018;
    /// Detach this terminal from the caller as controlling terminal.
    pub const SCTL_TTY_DETACH_CONTROLLING: u32 = 0x5354_0019;
}
use tty_ctl::*;

// Provide a static capabilities slice for TTY devices
static TTY_CAPS: [DeviceCapability; 1] = [DeviceCapability::Tty];

const DEFAULT_INTERRUPT_CHAR: u8 = 0x03; // Ctrl-C
const DEFAULT_QUIT_CHAR: u8 = 0x1C; // Ctrl-\
const DEFAULT_ERASE_CHAR: u8 = 0x7F; // DEL
const DEFAULT_EOF_CHAR: u8 = 0x04; // Ctrl-D
const DEFAULT_SUSPEND_CHAR: u8 = 0x1A; // Ctrl-Z
const DEFAULT_LITERAL_NEXT_CHAR: u8 = 0x16; // Ctrl-V

/// Terminal-generated control characters used by the line discipline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalControlChars {
    /// Interrupt character, delivered as a process-control interrupt event.
    pub interrupt: u8,
    /// Quit character, delivered as a process-control quit event.
    pub quit: u8,
    /// Erase character used by canonical input editing.
    pub erase: u8,
    /// End-of-file character used to complete a canonical input record.
    pub eof: u8,
    /// Suspend character, delivered as a terminal-stop process-control event.
    pub suspend: u8,
    /// Literal-next character used by extended canonical input editing.
    pub literal_next: u8,
}

/// Lower endpoint used by the TTY core for device output.
///
/// UART-backed console TTYs and future PTY slave TTYs both provide this
/// neutral byte-stream sink. Input still enters through the line discipline via
/// `inject_input_byte`.
pub trait TtyBackend: Send + Sync {
    /// Write bytes to the lower endpoint.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Bytes after TTY output processing.
    ///
    /// # Returns
    ///
    /// Number of bytes accepted by the lower endpoint.
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str>;

    /// Write one byte to the lower endpoint.
    ///
    /// # Arguments
    ///
    /// * `byte` - Byte after TTY output processing.
    ///
    /// # Returns
    ///
    /// Result indicating success or failure.
    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.write(&[byte]).map(|_| ())
    }

    /// Returns whether the lower endpoint can accept output.
    ///
    /// # Returns
    ///
    /// `true` when output can be written.
    fn can_write(&self) -> bool;
}

struct DeviceTtyBackend {
    device_id: usize,
}

impl DeviceTtyBackend {
    fn new(device_id: usize) -> Self {
        Self { device_id }
    }

    fn char_device(&self) -> Option<alloc::sync::Arc<dyn Device>> {
        DeviceManager::get_manager().get_device(self.device_id)
    }
}

impl TtyBackend for DeviceTtyBackend {
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        let Some(device) = self.char_device() else {
            return Err("TTY backend device not available");
        };
        let Some(char_device) = device.as_char_device() else {
            return Err("TTY backend device is not a character device");
        };
        char_device.write(buffer)
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        let Some(device) = self.char_device() else {
            return Err("TTY backend device not available");
        };
        let Some(char_device) = device.as_char_device() else {
            return Err("TTY backend device is not a character device");
        };
        char_device.write_byte(byte)
    }

    fn can_write(&self) -> bool {
        let Some(device) = self.char_device() else {
            return false;
        };
        device
            .as_char_device()
            .map(|char_device| char_device.can_write())
            .unwrap_or(false)
    }
}

/// TTY subsystem initialization
fn init_tty_subsystem() {
    let result = try_init_tty_subsystem();
    if let Err(e) = result {
        crate::early_println!("Failed to initialize TTY subsystem: {}", e);
    }
}

fn try_init_tty_subsystem() -> Result<(), &'static str> {
    let device_manager = DeviceManager::get_manager();

    // Prefer a Char device that advertises Serial capability (and is not itself a TTY)
    let devices_count = device_manager.get_devices_count();
    let mut serial_device_id: Option<usize> = None;
    for id in 1..=devices_count {
        if let Some(dev) = device_manager.get_device(id) {
            if dev.device_type() == DeviceType::Char
                && dev.capabilities().contains(&DeviceCapability::Serial)
                && !dev.capabilities().contains(&DeviceCapability::Tty)
            {
                serial_device_id = Some(id);
                break;
            }
        }
    }

    let uart_device_id =
        serial_device_id.ok_or("No Serial-capable char device found for TTY initialization")?;

    // Create TTY device with the resolved UART device ID
    let tty_device = Arc::new(TtyDevice::new("tty0", uart_device_id));
    tty_device.set_self_ref(Arc::downgrade(&tty_device));
    let uart_device = device_manager
        .get_device(uart_device_id)
        .ok_or("UART device not found")?;

    // Register TTY device as event listener for UART input events via capability
    if let Some(ec) = uart_device.as_event_capable() {
        let weak_tty = Arc::downgrade(&tty_device);
        ec.register_event_listener(weak_tty);
        crate::early_println!("TTY registered as UART event listener");
    }

    // Register TTY device with device manager under name tty0
    let _tty_id =
        device_manager.register_device_with_name(alloc::string::String::from("tty0"), tty_device);

    crate::early_println!("TTY subsystem initialized successfully");
    Ok(())
}

late_initcall!(init_tty_subsystem);

/// TTY device implementation.
///
/// This device provides terminal functionality including line discipline,
/// echo, and basic terminal I/O operations.
pub struct TtyDevice {
    name: &'static str,
    backend: Arc<dyn TtyBackend>,
    self_ref: spin::RwLock<Weak<TtyDevice>>,

    // Input buffer for line discipline
    input_buffer: Arc<Mutex<VecDeque<u8>>>,

    // Waker for blocking reads
    input_waker: Waker,

    // Line discipline flags (OS/ABI-neutral)
    canonical_mode: AtomicBool,
    echo_enabled: AtomicBool,
    signal_chars_enabled: AtomicBool,
    crnl_input_enabled: AtomicBool,
    output_postprocess_enabled: AtomicBool,
    extended_input_enabled: AtomicBool,
    canonical_eof_ready: AtomicBool,
    literal_next_pending: AtomicBool,
    interrupt_char: AtomicU8,
    quit_char: AtomicU8,
    erase_char: AtomicU8,
    eof_char: AtomicU8,
    suspend_char: AtomicU8,
    literal_next_char: AtomicU8,
    tostop_enabled: AtomicBool,

    // Neutral read policy: minimum bytes to return and optional timeout (ms)
    read_min_ready_bytes: AtomicU16,
    read_timeout_ms: AtomicU16,

    // Window size in character cells (OS/ABI-neutral)
    winsize_cols: Mutex<u16>,
    winsize_rows: Mutex<u16>,
    // Debug logging flag
    debug_enabled: AtomicBool,

    // Keyboard mode (0=XLATE, 1=MEDIUMRAW, 2=RAW)
    kb_mode: core::sync::atomic::AtomicU8,

    // Simple escape sequence parser state for arrow keys in raw-ish mode
    // 0: none, 1: got ESC (0x1B), 2: got ESC '[', 3: got ESC 'O'
    esc_state: Mutex<u8>,
    // Per-device non-blocking I/O flag (shared by all FDs referencing this TTY)
    nonblocking: AtomicBool,
    exclusive: AtomicBool,
    foreground_task_group_id: Mutex<Option<usize>>,
    controlling_session_id: Mutex<Option<usize>>,
    // Serializes write operations (Linux tty_struct::atomic_write_lock equivalent)
    write_lock: Mutex<()>,
}

impl TtyDevice {
    /// Create a TTY backed by an existing character device.
    ///
    /// # Arguments
    ///
    /// * `name` - Static device name reported by the TTY.
    /// * `uart_device_id` - DeviceManager ID for the lower serial endpoint.
    ///
    /// # Returns
    ///
    /// A new TTY device with default line discipline settings.
    pub fn new(name: &'static str, uart_device_id: usize) -> Self {
        Self::new_with_backend(name, Arc::new(DeviceTtyBackend::new(uart_device_id)))
    }

    /// Create a TTY backed by a custom lower endpoint.
    ///
    /// # Arguments
    ///
    /// * `name` - Static device name reported by the TTY.
    /// * `backend` - Lower endpoint used for TTY output.
    ///
    /// # Returns
    ///
    /// A new TTY device with default line discipline settings.
    pub fn new_with_backend(name: &'static str, backend: Arc<dyn TtyBackend>) -> Self {
        Self {
            name,
            backend,
            self_ref: spin::RwLock::new(Weak::new()),
            input_buffer: Arc::new(Mutex::new(VecDeque::new())),
            input_waker: Waker::new_interruptible("tty_input"),
            canonical_mode: AtomicBool::new(true),
            echo_enabled: AtomicBool::new(true),
            signal_chars_enabled: AtomicBool::new(true),
            crnl_input_enabled: AtomicBool::new(true),
            output_postprocess_enabled: AtomicBool::new(true),
            extended_input_enabled: AtomicBool::new(true),
            canonical_eof_ready: AtomicBool::new(false),
            literal_next_pending: AtomicBool::new(false),
            interrupt_char: AtomicU8::new(DEFAULT_INTERRUPT_CHAR),
            quit_char: AtomicU8::new(DEFAULT_QUIT_CHAR),
            erase_char: AtomicU8::new(DEFAULT_ERASE_CHAR),
            eof_char: AtomicU8::new(DEFAULT_EOF_CHAR),
            suspend_char: AtomicU8::new(DEFAULT_SUSPEND_CHAR),
            literal_next_char: AtomicU8::new(DEFAULT_LITERAL_NEXT_CHAR),
            tostop_enabled: AtomicBool::new(false),
            read_min_ready_bytes: AtomicU16::new(1),
            read_timeout_ms: AtomicU16::new(0),
            winsize_cols: Mutex::new(80),
            winsize_rows: Mutex::new(25),
            // Disable per-byte debug logging by default (can be enabled via SCTL_TTY_SET_DEBUG)
            debug_enabled: AtomicBool::new(false),
            kb_mode: core::sync::atomic::AtomicU8::new(0),
            esc_state: Mutex::new(0),
            nonblocking: AtomicBool::new(false),
            exclusive: AtomicBool::new(false),
            foreground_task_group_id: Mutex::new(None),
            controlling_session_id: Mutex::new(None),
            write_lock: Mutex::new(()),
        }
    }

    pub fn set_self_ref(&self, weak: Weak<TtyDevice>) {
        *self.self_ref.write() = weak;
    }

    pub fn weak_self(&self) -> Option<Weak<TtyDevice>> {
        let weak = self.self_ref.read().clone();
        weak.upgrade().map(|_| weak)
    }

    pub fn set_controlling_session_id(&self, session_id: usize) {
        *self.controlling_session_id.lock() = Some(session_id);
    }

    pub fn get_controlling_session_id(&self) -> Option<usize> {
        *self.controlling_session_id.lock()
    }

    pub fn clear_controlling_session_id(&self, session_id: usize) {
        let mut guard = self.controlling_session_id.lock();
        if *guard == Some(session_id) {
            *guard = None;
        }
    }

    pub fn acquire_as_controlling_tty(&self, force: bool) -> Result<(), &'static str> {
        let task = mytask().ok_or("no current task")?;

        if let Some(current_tty) = task.get_controlling_tty()
            && let Some(this_tty) = self.weak_self().and_then(|weak| weak.upgrade())
            && Arc::ptr_eq(&current_tty, &this_tty)
        {
            return Ok(());
        }

        if task.get_controlling_tty().is_some() || !task.is_session_leader() {
            return Err("task cannot acquire controlling terminal");
        }

        let session_id = task.get_session_id();
        if let Some(owner_session_id) = self.get_controlling_session_id()
            && owner_session_id != session_id
            && !force
        {
            return Err("terminal already controlled by another session");
        }

        let weak = self
            .weak_self()
            .ok_or("terminal self reference unavailable")?;
        task.set_controlling_tty(Some(weak));
        self.set_controlling_session_id(session_id);
        if self.get_foreground_task_group_id().is_none() {
            self.set_foreground_task_group_id(task.get_process_group_id());
        }
        Ok(())
    }

    pub fn detach_controlling_tty(&self) -> Result<(), &'static str> {
        let task = mytask().ok_or("no current task")?;
        let Some(current_tty) = task.get_controlling_tty() else {
            return Ok(());
        };
        let Some(this_tty) = self.weak_self().and_then(|weak| weak.upgrade()) else {
            return Err("terminal self reference unavailable");
        };
        if !Arc::ptr_eq(&current_tty, &this_tty) {
            return Err("terminal is not caller's controlling terminal");
        }

        task.clear_controlling_tty();
        if task.is_session_leader() {
            self.clear_controlling_session_id(task.get_session_id());
        }
        Ok(())
    }

    pub fn set_exclusive(&self, enabled: bool) {
        self.exclusive.store(enabled, Ordering::Relaxed);
    }

    pub fn is_exclusive(&self) -> bool {
        self.exclusive.load(Ordering::Relaxed)
    }

    pub fn inject_input_byte(&self, byte: u8) {
        self.handle_input_byte(byte);
    }

    pub fn set_tostop(&self, enabled: bool) {
        self.tostop_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_tostop_enabled(&self) -> bool {
        self.tostop_enabled.load(Ordering::Relaxed)
    }

    pub fn set_signal_chars_enabled(&self, enabled: bool) {
        self.signal_chars_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_signal_chars_enabled(&self) -> bool {
        self.signal_chars_enabled.load(Ordering::Relaxed)
    }

    pub fn set_crnl_input_enabled(&self, enabled: bool) {
        self.crnl_input_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_crnl_input_enabled(&self) -> bool {
        self.crnl_input_enabled.load(Ordering::Relaxed)
    }

    pub fn set_output_postprocess_enabled(&self, enabled: bool) {
        self.output_postprocess_enabled
            .store(enabled, Ordering::Relaxed);
    }

    pub fn is_output_postprocess_enabled(&self) -> bool {
        self.output_postprocess_enabled.load(Ordering::Relaxed)
    }

    pub fn set_extended_input_enabled(&self, enabled: bool) {
        self.extended_input_enabled
            .store(enabled, Ordering::Relaxed);
        if !enabled {
            self.literal_next_pending.store(false, Ordering::Relaxed);
        }
    }

    pub fn is_extended_input_enabled(&self) -> bool {
        self.extended_input_enabled.load(Ordering::Relaxed)
    }

    /// Set terminal-generated control characters.
    ///
    /// A zero byte disables the corresponding special meaning, matching the
    /// Linux ABI adapter's current `_POSIX_VDISABLE` behavior.
    pub fn set_control_chars(&self, chars: TerminalControlChars) {
        self.interrupt_char
            .store(chars.interrupt, Ordering::Relaxed);
        self.quit_char.store(chars.quit, Ordering::Relaxed);
        self.erase_char.store(chars.erase, Ordering::Relaxed);
        self.eof_char.store(chars.eof, Ordering::Relaxed);
        self.suspend_char.store(chars.suspend, Ordering::Relaxed);
        self.literal_next_char
            .store(chars.literal_next, Ordering::Relaxed);
    }

    /// Return the current terminal-generated control characters.
    pub fn get_control_chars(&self) -> TerminalControlChars {
        TerminalControlChars {
            interrupt: self.interrupt_char.load(Ordering::Relaxed),
            quit: self.quit_char.load(Ordering::Relaxed),
            erase: self.erase_char.load(Ordering::Relaxed),
            eof: self.eof_char.load(Ordering::Relaxed),
            suspend: self.suspend_char.load(Ordering::Relaxed),
            literal_next: self.literal_next_char.load(Ordering::Relaxed),
        }
    }

    /// Block until the TTY input buffer becomes non-empty.
    /// Used by polling syscalls (e.g. pselect6) to implement blocking semantics.
    pub fn wait_until_readable(&self, trapframe: &mut Trapframe) {
        loop {
            if self.can_read() {
                return;
            }
            if let Some(task) = mytask() {
                self.input_waker.wait(task.get_id(), trapframe);
            } else {
                // No task context; abort wait
                return;
            }
        }
    }

    /// Wake all tasks waiting for TTY input readiness.
    /// This is used by timeout handlers to preempt a blocking wait.
    pub fn wake_input(&self) {
        self.input_waker.wake_all();
    }

    /// Wait until readable with a timeout (in ticks).
    /// Returns true if timed out, false if input became available.
    pub fn wait_until_readable_with_timeout_ticks(
        &self,
        trapframe: &mut Trapframe,
        ticks: u64,
    ) -> bool {
        if self.can_read() {
            return false;
        }
        if ticks == 0 {
            return true;
        }

        struct TtyTimeoutHandler {
            tty_ptr: *const TtyDevice,
        }
        unsafe impl Send for TtyTimeoutHandler {}
        unsafe impl Sync for TtyTimeoutHandler {}
        impl TimerHandler for TtyTimeoutHandler {
            fn on_timer_expired(self: Arc<Self>, _context: usize) {
                unsafe {
                    (*self.tty_ptr).wake_input();
                }
            }
        }

        let deadline = get_tick().saturating_add(ticks);
        let handler: Arc<dyn TimerHandler> = Arc::new(TtyTimeoutHandler {
            tty_ptr: self as *const TtyDevice,
        });
        let timer_id = add_timer(deadline, &handler, 0);

        if let Some(task) = mytask() {
            self.input_waker.wait(task.get_id(), trapframe);
        }

        // After wake: cancel timer if still queued and decide reason
        cancel_timer(timer_id);
        if self.can_read() { false } else { true }
    }

    /// Current buffered input length (for diagnostics / readiness debugging)
    pub fn input_len(&self) -> usize {
        self.input_buffer.lock().len()
    }

    pub fn set_foreground_task_group_id(&self, task_group_id: usize) {
        *self.foreground_task_group_id.lock() = Some(task_group_id);
    }

    pub fn get_foreground_task_group_id(&self) -> Option<usize> {
        *self.foreground_task_group_id.lock()
    }

    fn resolve_foreground_task_group_id(&self, user_task_group_id: usize) -> usize {
        let Some(caller) = mytask() else {
            return user_task_group_id;
        };
        let Some(global_task_id) = caller.get_namespace().resolve_global_id(user_task_group_id)
        else {
            return user_task_group_id;
        };
        crate::sched::scheduler::get_task_by_id(global_task_id)
            .map(|task| task.get_process_group_id())
            .unwrap_or(user_task_group_id)
    }

    fn user_visible_foreground_task_group_id(&self, task_group_id: usize) -> usize {
        let Some(caller) = mytask() else {
            return task_group_id;
        };
        caller
            .get_namespace()
            .resolve_local_id(task_group_id)
            .unwrap_or(task_group_id)
    }

    fn send_process_control_to_foreground(&self, ptype: crate::ipc::event::ProcessControlType) {
        use crate::ipc::event::{
            Event, EventContent, EventManager, EventPayload, EventPriority, GroupTarget,
        };

        let Some(process_group_id) = self.get_foreground_task_group_id() else {
            return;
        };

        let event = Event::group(
            GroupTarget::TaskGroup(process_group_id as u32),
            EventContent::ProcessControl(ptype),
            EventPriority::High,
            true,
            EventPayload::Empty,
        );
        let _ = EventManager::get_manager().send_event(event);
    }

    fn send_process_control_to_current_group(&self, ptype: crate::ipc::event::ProcessControlType) {
        use crate::ipc::event::{
            Event, EventContent, EventManager, EventPayload, EventPriority, GroupTarget,
        };

        let Some(task) = mytask() else {
            return;
        };

        let event = Event::group(
            GroupTarget::TaskGroup(task.get_process_group_id() as u32),
            EventContent::ProcessControl(ptype),
            EventPriority::High,
            true,
            EventPayload::Empty,
        );
        let _ = EventManager::get_manager().send_event(event);
    }

    fn is_current_process_group_background(&self) -> bool {
        let Some(task) = mytask() else {
            return false;
        };
        let Some(controlling_tty) = task.get_controlling_tty() else {
            return false;
        };
        let Some(this_tty) = self.weak_self().and_then(|weak| weak.upgrade()) else {
            return false;
        };
        if !Arc::ptr_eq(&controlling_tty, &this_tty) {
            return false;
        }

        match self.get_foreground_task_group_id() {
            Some(foreground_pgid) => foreground_pgid != task.get_process_group_id(),
            None => false,
        }
    }

    /// Read readiness for select/poll semantics.
    /// - Canonical mode: ready only when a full line (ending with '\n') exists.
    /// - Non-canonical: honor read_min_ready_bytes; in RAW mode head 0xE0 requires a pair.
    pub fn is_read_ready_for_select(&self) -> bool {
        if self.canonical_mode.load(Ordering::Relaxed) {
            let g = self.input_buffer.lock();
            return self.canonical_eof_ready.load(Ordering::Relaxed)
                || g.iter().any(|&b| b == b'\n');
        }

        let min_ready = self.read_min_ready_bytes.load(Ordering::Relaxed) as usize;
        let kb_mode = self.kb_mode.load(Ordering::Relaxed);

        // If min_ready == 0, read() is defined to return immediately
        // (possibly 0 bytes), so this is non-blocking -> ready.
        if min_ready == 0 {
            return true;
        }

        if kb_mode == 2 {
            let g = self.input_buffer.lock();
            let available = g.len();
            if let Some(&first) = g.front() {
                if first == 0xE0 {
                    if available < 2 {
                        return false;
                    }
                    return available >= min_ready;
                }
            } else {
                return false;
            }
            available >= min_ready
        } else {
            let available = self.input_len();
            available >= min_ready
        }
    }

    /// Handle input byte from UART device.
    ///
    /// This method processes incoming bytes and applies line discipline.
    fn handle_input_byte(&self, mut byte: u8) {
        if self.debug_enabled.load(Ordering::Relaxed) {
            crate::println!(
                "[TTY] RX byte=0x{:02x} '{}' canonical={} size={}",
                byte,
                if byte.is_ascii_graphic() || byte == b' ' {
                    byte as char
                } else {
                    '.'
                },
                self.canonical_mode.load(Ordering::Relaxed),
                self.input_buffer.lock().len()
            );
        }
        if self.crnl_input_enabled.load(Ordering::Relaxed) && byte == b'\r' {
            byte = b'\n';
        }

        let control_chars = self.get_control_chars();
        if self.literal_next_pending.swap(false, Ordering::Relaxed) {
            if self.echo_enabled.load(Ordering::Relaxed) {
                self.echo_char(byte);
            }
            let wake = !self.canonical_mode.load(Ordering::Relaxed) || byte == b'\n';
            let mut input_buffer = self.input_buffer.lock();
            input_buffer.push_back(byte);
            drop(input_buffer);
            if wake {
                self.input_waker.wake_all();
            }
            return;
        }

        if self.signal_chars_enabled.load(Ordering::Relaxed)
            && self.kb_mode.load(Ordering::Relaxed) == 0
        {
            match byte {
                b if control_chars.interrupt != 0 && b == control_chars.interrupt => {
                    if self.echo_enabled.load(Ordering::Relaxed) {
                        self.echo_control_char(control_chars.interrupt);
                        self.echo_char('\r' as u8);
                        self.echo_char('\n' as u8);
                    }
                    self.send_process_control_to_foreground(
                        crate::ipc::event::ProcessControlType::Interrupt,
                    );
                    return;
                }
                b if control_chars.quit != 0 && b == control_chars.quit => {
                    if self.echo_enabled.load(Ordering::Relaxed) {
                        self.echo_control_char(control_chars.quit);
                        self.echo_char('\r' as u8);
                        self.echo_char('\n' as u8);
                    }
                    self.send_process_control_to_foreground(
                        crate::ipc::event::ProcessControlType::Quit,
                    );
                    return;
                }
                b if control_chars.suspend != 0 && b == control_chars.suspend => {
                    if self.echo_enabled.load(Ordering::Relaxed) {
                        self.echo_control_char(control_chars.suspend);
                        self.echo_char('\r' as u8);
                        self.echo_char('\n' as u8);
                    }
                    self.send_process_control_to_foreground(
                        crate::ipc::event::ProcessControlType::TerminalStop,
                    );
                    return;
                }
                _ => {}
            }
        }

        // Canonical mode processing
        if self.canonical_mode.load(Ordering::Relaxed) {
            match byte {
                b if self.extended_input_enabled.load(Ordering::Relaxed)
                    && control_chars.literal_next != 0
                    && b == control_chars.literal_next =>
                {
                    self.literal_next_pending.store(true, Ordering::Relaxed);
                }
                b if control_chars.eof != 0 && b == control_chars.eof => {
                    self.canonical_eof_ready.store(true, Ordering::Relaxed);
                    self.input_waker.wake_all();
                }
                // Backspace/DEL
                0x08 | 0x7F
                    if control_chars.erase == DEFAULT_ERASE_CHAR || control_chars.erase == 0x08 =>
                {
                    let mut input_buffer = self.input_buffer.lock();
                    if input_buffer.pop_back().is_some()
                        && self.echo_enabled.load(Ordering::Relaxed)
                    {
                        self.echo_backspace();
                    }
                }
                b if control_chars.erase != 0 && b == control_chars.erase => {
                    let mut input_buffer = self.input_buffer.lock();
                    if input_buffer.pop_back().is_some()
                        && self.echo_enabled.load(Ordering::Relaxed)
                    {
                        self.echo_backspace();
                    }
                }
                // Line feed
                b'\n' => {
                    if self.echo_enabled.load(Ordering::Relaxed) {
                        self.echo_char(b'\r');
                        self.echo_char(b'\n');
                    }
                    let mut input_buffer = self.input_buffer.lock();
                    input_buffer.push_back(b'\n');
                    drop(input_buffer);
                    self.input_waker.wake_all();
                }
                // Regular characters
                byte => {
                    if self.echo_enabled.load(Ordering::Relaxed) {
                        self.echo_char(byte);
                    }
                    let mut input_buffer = self.input_buffer.lock();
                    input_buffer.push_back(byte);
                    drop(input_buffer);
                    if !self.canonical_mode.load(Ordering::Relaxed) {
                        self.input_waker.wake_all();
                    }
                }
            }
        } else {
            // Non-canonical path: honor keyboard mode
            let kb_mode = self.kb_mode.load(Ordering::Relaxed);
            if kb_mode == 0 {
                // XLATE-like: pass through raw byte
                let mut input_buffer = self.input_buffer.lock();
                input_buffer.push_back(byte);
                drop(input_buffer);
                self.input_waker.wake_all();
            } else if kb_mode == 1 {
                // MEDIUMRAW: return 1-byte Linux keycode (press-only)
                fn ascii_to_linux_keycode(b: u8) -> Option<u8> {
                    match b {
                        b'1' => Some(2),
                        b'2' => Some(3),
                        b'3' => Some(4),
                        b'4' => Some(5),
                        b'5' => Some(6),
                        b'6' => Some(7),
                        b'7' => Some(8),
                        b'8' => Some(9),
                        b'9' => Some(10),
                        b'0' => Some(11),
                        b'-' => Some(12),
                        b'=' => Some(13),
                        0x08 | 0x7F => Some(14), // Backspace
                        b'\t' => Some(15),
                        b'q' => Some(16),
                        b'w' => Some(17),
                        b'e' => Some(18),
                        b'r' => Some(19),
                        b't' => Some(20),
                        b'y' => Some(21),
                        b'u' => Some(22),
                        b'i' => Some(23),
                        b'o' => Some(24),
                        b'p' => Some(25),
                        b'[' => Some(26),
                        b']' => Some(27),
                        b'\n' | b'\r' => Some(28),
                        b'a' => Some(30),
                        b's' => Some(31),
                        b'd' => Some(32),
                        b'f' => Some(33),
                        b'g' => Some(34),
                        b'h' => Some(35),
                        b'j' => Some(36),
                        b'k' => Some(37),
                        b'l' => Some(38),
                        b';' => Some(39),
                        b'\'' => Some(40),
                        b'`' => Some(41),
                        b'\\' => Some(43),
                        b'z' => Some(44),
                        b'x' => Some(45),
                        b'c' => Some(46),
                        b'v' => Some(47),
                        b'b' => Some(48),
                        b'n' => Some(49),
                        b'm' => Some(50),
                        b',' => Some(51),
                        b'.' => Some(52),
                        b'/' => Some(53),
                        b' ' => Some(57),
                        // Uppercase letters -> map to same keycode as lowercase
                        b'A'..=b'Z' => Some(30 + (b.to_ascii_lowercase() - b'a') as u8),
                        _ => None,
                    }
                }
                fn push_keycode_press_release(dev: &TtyDevice, code: u8) {
                    let mut input_buffer = dev.input_buffer.lock();
                    input_buffer.push_back(code);
                    input_buffer.push_back(code | 0x80); // release
                    drop(input_buffer);
                    dev.input_waker.wake_all();
                }
                // Interpret ESC/CSI/SS3 as Linux keycodes
                let mut handled_escape = false;
                {
                    let mut st = self.esc_state.lock();
                    match (*st, byte) {
                        (0, 0x1B) => {
                            *st = 1;
                            handled_escape = true;
                        }
                        (1, b'[') => {
                            *st = 2;
                            handled_escape = true;
                        }
                        (1, b'O') => {
                            *st = 3;
                            handled_escape = true;
                        }
                        // Arrows (CSI)
                        (2, b'A') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 103);
                        } // Up
                        (2, b'B') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 108);
                        } // Down
                        (2, b'C') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 106);
                        } // Right
                        (2, b'D') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 105);
                        } // Left
                        // Arrows (SS3)
                        (3, b'H') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 103);
                        }
                        (3, b'P') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 108);
                        }
                        (3, b'M') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 106);
                        }
                        (3, b'K') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 105);
                        }
                        // Home/End (CSI)
                        (2, b'H') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 102);
                        } // Home
                        (2, b'F') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 107);
                        } // End
                        // Home/End (SS3)
                        (3, b'F') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 107);
                        }
                        // CSI numeric ~ sequences
                        (2, b'2') => {
                            *st = 4;
                            handled_escape = true;
                        } // -> Insert
                        (2, b'3') => {
                            *st = 5;
                            handled_escape = true;
                        } // -> Delete
                        (2, b'5') => {
                            *st = 6;
                            handled_escape = true;
                        } // -> PageUp
                        (2, b'6') => {
                            *st = 7;
                            handled_escape = true;
                        } // -> PageDown
                        (4, b'~') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 110);
                        } // Insert
                        (5, b'~') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 111);
                        } // Delete
                        (6, b'~') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 104);
                        } // PageUp
                        (7, b'~') => {
                            *st = 0;
                            handled_escape = true;
                            push_keycode_press_release(self, 109);
                        } // PageDown
                        (1, _) | (2, _) | (3, _) => {
                            *st = 0;
                        }
                        _ => {}
                    }
                }
                if !handled_escape {
                    if let Some(code) = ascii_to_linux_keycode(byte) {
                        push_keycode_press_release(self, code);
                    }
                }
            } else {
                // RAW: XT Set1 scancodes (extended codes are E0-prefixed)
                fn ascii_to_set1_scancode(b: u8) -> Option<u8> {
                    match b {
                        b'1' => Some(2),
                        b'2' => Some(3),
                        b'3' => Some(4),
                        b'4' => Some(5),
                        b'5' => Some(6),
                        b'6' => Some(7),
                        b'7' => Some(8),
                        b'8' => Some(9),
                        b'9' => Some(10),
                        b'0' => Some(11),
                        b'-' => Some(12),
                        b'=' => Some(13),
                        0x08 | 0x7F => Some(14),
                        b'\t' => Some(15),
                        b'q' => Some(16),
                        b'w' => Some(17),
                        b'e' => Some(18),
                        b'r' => Some(19),
                        b't' => Some(20),
                        b'y' => Some(21),
                        b'u' => Some(22),
                        b'i' => Some(23),
                        b'o' => Some(24),
                        b'p' => Some(25),
                        b'[' => Some(26),
                        b']' => Some(27),
                        b'\n' | b'\r' => Some(28),
                        b'a' => Some(30),
                        b's' => Some(31),
                        b'd' => Some(32),
                        b'f' => Some(33),
                        b'g' => Some(34),
                        b'h' => Some(35),
                        b'j' => Some(36),
                        b'k' => Some(37),
                        b'l' => Some(38),
                        b';' => Some(39),
                        b'\'' => Some(40),
                        b'`' => Some(41),
                        b'\\' => Some(43),
                        b'z' => Some(44),
                        b'x' => Some(45),
                        b'c' => Some(46),
                        b'v' => Some(47),
                        b'b' => Some(48),
                        b'n' => Some(49),
                        b'm' => Some(50),
                        b',' => Some(51),
                        b'.' => Some(52),
                        b'/' => Some(53),
                        b' ' => Some(57),
                        b'A'..=b'Z' => Some(30 + (b.to_ascii_lowercase() - b'a') as u8),
                        _ => None,
                    }
                }
                fn push_scancode_press_release(dev: &TtyDevice, code: u8, extended: bool) {
                    let mut input_buffer = dev.input_buffer.lock();
                    // press
                    if extended {
                        input_buffer.push_back(0xE0);
                    }
                    input_buffer.push_back(code);
                    // release
                    if extended {
                        input_buffer.push_back(0xE0);
                    }
                    input_buffer.push_back(code | 0x80);
                    drop(input_buffer);
                    dev.input_waker.wake_all();
                }
                let mut handled_escape = false;
                {
                    let mut st = self.esc_state.lock();
                    match (*st, byte) {
                        (0, 0x1B) => {
                            *st = 1;
                            handled_escape = true;
                        }
                        (1, b'[') => {
                            *st = 2;
                            handled_escape = true;
                        }
                        (1, b'O') => {
                            *st = 3;
                            handled_escape = true;
                        }
                        // Arrows via CSI
                        (2, b'A') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x48, true);
                        } // Up   E0 48
                        (2, b'B') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x50, true);
                        } // Down E0 50
                        (2, b'C') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x4D, true);
                        } // Right E0 4D
                        (2, b'D') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x4B, true);
                        } // Left  E0 4B
                        // Arrows via SS3 keypad-style (DEC application keypad): O H/K/M/P etc.
                        (3, b'H') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x48, true);
                        } // KP-8 = Up
                        (3, b'P') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x50, true);
                        } // KP-2 = Down
                        (3, b'M') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x4D, true);
                        } // KP-6 = Right
                        (3, b'K') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x4B, true);
                        } // KP-4 = Left
                        // Home/End via CSI
                        (2, b'H') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x47, true);
                        } // Home  E0 47
                        (2, b'F') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x4F, true);
                        } // End   E0 4F
                        // Home/End via SS3 (common mappings)
                        (3, b'F') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x4F, true);
                        }
                        // CSI numeric ~ sequences
                        (2, b'2') => {
                            *st = 4;
                            handled_escape = true;
                        } // expect '~' => Insert
                        (2, b'3') => {
                            *st = 5;
                            handled_escape = true;
                        } // expect '~' => Delete
                        (2, b'5') => {
                            *st = 6;
                            handled_escape = true;
                        } // expect '~' => PageUp
                        (2, b'6') => {
                            *st = 7;
                            handled_escape = true;
                        } // expect '~' => PageDown
                        (4, b'~') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x52, true);
                        } // Ins  E0 52
                        (5, b'~') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x53, true);
                        } // Del  E0 53
                        (6, b'~') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x49, true);
                        } // PgUp E0 49
                        (7, b'~') => {
                            *st = 0;
                            handled_escape = true;
                            push_scancode_press_release(self, 0x51, true);
                        } // PgDn E0 51
                        (1, _) | (2, _) | (3, _) => {
                            // Unknown sequence, reset and fall through to ASCII mapping
                            *st = 0;
                        }
                        _ => {}
                    }
                }
                if !handled_escape {
                    if let Some(code) = ascii_to_set1_scancode(byte) {
                        // Non-extended: generate press/release
                        let mut input_buffer = self.input_buffer.lock();
                        input_buffer.push_back(code);
                        input_buffer.push_back(code | 0x80);
                        drop(input_buffer);
                        self.input_waker.wake_all();
                    }
                }
            }
        }
    }

    /// Echo character back to output.
    fn echo_char(&self, byte: u8) {
        let _lock = self.write_lock.lock();
        let _ = self.backend.write_byte(byte);
    }

    /// Echo backspace sequence.
    fn echo_backspace(&self) {
        // Backspace echo: BS + space + BS
        self.echo_char(0x08);
        self.echo_char(b' ');
        self.echo_char(0x08);
    }

    fn echo_control_char(&self, byte: u8) {
        self.echo_char(b'^');
        self.echo_char(match byte {
            0x00..=0x1F => byte + 0x40,
            0x7F => b'?',
            _ => byte,
        });
    }
}

impl Selectable for TtyDevice {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        if interest.read {
            set.read = self.is_read_ready_for_select();
        }
        if interest.write {
            // TTY writes are considered ready (no internal backpressure yet)
            set.write = true;
        }
        if interest.except {
            set.except = false;
        }
        set
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        // Only read interest requires actual waiting; write/except treated as always-ready
        if interest.read {
            match timeout_ticks {
                Some(ticks) => {
                    let timed_out = self.wait_until_readable_with_timeout_ticks(trapframe, ticks);
                    if timed_out {
                        SelectWaitOutcome::TimedOut
                    } else {
                        SelectWaitOutcome::Ready
                    }
                }
                None => {
                    self.wait_until_readable(trapframe);
                    SelectWaitOutcome::Ready
                }
            }
        } else {
            SelectWaitOutcome::Ready
        }
    }

    fn set_nonblocking(&self, enabled: bool) {
        crate::println!("[TTY] set_nonblocking: {}", enabled);
        self.nonblocking.store(enabled, Ordering::Relaxed);
    }

    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Relaxed)
    }
}

impl TtyControl for TtyDevice {
    fn set_echo(&self, enabled: bool) {
        self.echo_enabled.store(enabled, Ordering::Relaxed);
    }
    fn is_echo_enabled(&self) -> bool {
        self.echo_enabled.load(Ordering::Relaxed)
    }

    fn set_canonical(&self, enabled: bool) {
        self.canonical_mode.store(enabled, Ordering::Relaxed);
    }
    fn is_canonical(&self) -> bool {
        self.canonical_mode.load(Ordering::Relaxed)
    }

    fn set_winsize(&self, cols: u16, rows: u16) {
        let changed = {
            let mut current_cols = self.winsize_cols.lock();
            let mut current_rows = self.winsize_rows.lock();
            let changed = *current_cols != cols || *current_rows != rows;
            *current_cols = cols;
            *current_rows = rows;
            changed
        };

        if changed {
            self.send_process_control_to_foreground(
                crate::ipc::event::ProcessControlType::WindowChange,
            );
        }
    }
    fn get_winsize(&self) -> (u16, u16) {
        (*self.winsize_cols.lock(), *self.winsize_rows.lock())
    }
}

impl DeviceEventListener for TtyDevice {
    fn on_device_event(&self, event: &dyn DeviceEvent) {
        if let Some(input_event) = event.as_any().downcast_ref::<InputEvent>() {
            self.handle_input_byte(input_event.data);
        }
    }

    fn interested_in(&self, event_type: &str) -> bool {
        event_type == "input"
    }
}

impl MemoryMappingOps for TtyDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by TTY device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // TTY devices don't support memory mapping
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // TTY devices don't support memory mapping
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Device for TtyDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }

    fn capabilities(&self) -> &'static [DeviceCapability] {
        &TTY_CAPS
    }
}

impl CharDevice for TtyDevice {
    fn read(&self, buffer: &mut [u8]) -> usize {
        if buffer.is_empty() {
            return 0;
        }
        if self.is_current_process_group_background() {
            self.send_process_control_to_current_group(
                crate::ipc::event::ProcessControlType::TerminalInput,
            );
            return 0;
        }

        // Fast path: non-blocking immediate return if policy requires 0 and no data
        let min_ready = self.read_min_ready_bytes.load(Ordering::Relaxed) as usize;
        let canonical = self.canonical_mode.load(Ordering::Relaxed);

        // Helper to copy out up to buffer.len() from input buffer
        let copy_out = |buf: &mut [u8], until_newline: bool| -> usize {
            let mut bytes = 0;
            let mut guard = self.input_buffer.lock();
            if until_newline {
                // Find newline position
                if let Some(pos) = guard.iter().position(|&b| b == b'\n') {
                    // Include the newline itself
                    let take = core::cmp::min(pos + 1, buf.len());
                    for i in 0..take {
                        if let Some(b) = guard.pop_front() {
                            buf[i] = b;
                            bytes += 1;
                        } else {
                            break;
                        }
                    }
                }
            } else {
                while bytes < buf.len() {
                    if let Some(b) = guard.pop_front() {
                        buf[bytes] = b;
                        bytes += 1;
                    } else {
                        break;
                    }
                }
            }
            bytes
        };

        // Canonical mode: block until a full line (ending with '\n') is available
        if canonical {
            loop {
                // If a newline exists, copy out a line chunk. EOF completes the
                // current record without appending a byte; with an empty record
                // this returns 0.
                let (has_newline, has_eof) = {
                    let g = self.input_buffer.lock();
                    (
                        g.iter().any(|&b| b == b'\n'),
                        self.canonical_eof_ready.load(Ordering::Relaxed),
                    )
                };
                if has_newline {
                    return copy_out(buffer, true);
                }
                if has_eof {
                    self.canonical_eof_ready.store(false, Ordering::Relaxed);
                    let mut bytes = 0;
                    let mut guard = self.input_buffer.lock();
                    while bytes < buffer.len() {
                        if let Some(b) = guard.pop_front() {
                            buffer[bytes] = b;
                            bytes += 1;
                        } else {
                            break;
                        }
                    }
                    return bytes;
                }
                // Wait for more input
                if self.nonblocking.load(Ordering::Relaxed) {
                    return 0; // non-blocking: no data yet
                }
                if let Some(task) = mytask() {
                    self.input_waker.wait(task.get_id(), task.get_trapframe());
                } else {
                    // No task context; return nothing
                    return 0;
                }
            }
        }

        // Non-canonical (raw-ish): honor min_ready_bytes policy, and group E0-prefixed scancodes
        if min_ready == 0 {
            // Immediate return with whatever is available (possibly 0)
            // For RAW prefer to return E0 2-byte pairs atomically
            let kb_mode = self.kb_mode.load(Ordering::Relaxed);
            if kb_mode == 2 {
                let mut guard = self.input_buffer.lock();
                if let Some(&first) = guard.front() {
                    if first == 0xE0 {
                        if guard.len() >= 2 && buffer.len() >= 2 {
                            let b0 = guard.pop_front().unwrap();
                            let b1 = guard.pop_front().unwrap();
                            drop(guard);
                            buffer[0] = b0;
                            buffer[1] = b1;
                            return 2;
                        }
                    }
                }
                drop(guard);
            }
            return copy_out(buffer, false);
        }

        loop {
            let kb_mode = self.kb_mode.load(Ordering::Relaxed);
            if kb_mode == 2 {
                // RAW: if head is E0 and caller expects 2 bytes, wait for pair.
                let (need_pair, have_pair) = {
                    let g = self.input_buffer.lock();
                    let head_is_e0 = g.front().map(|b| *b == 0xE0).unwrap_or(false);
                    let have = g.len() >= 2;
                    (head_is_e0 && buffer.len() >= 2, have)
                };
                if need_pair && !have_pair {
                    if self.nonblocking.load(Ordering::Relaxed) {
                        return 0;
                    }
                    if let Some(task) = mytask() {
                        self.input_waker.wait(task.get_id(), task.get_trapframe());
                        continue;
                    } else {
                        return 0;
                    }
                }
            }

            let available = { self.input_buffer.lock().len() };
            if available >= core::cmp::min(min_ready as usize, buffer.len()) {
                // Try to return E0 pair atomically if possible
                let kb_mode = self.kb_mode.load(Ordering::Relaxed);
                if kb_mode == 2 {
                    let mut guard = self.input_buffer.lock();
                    if let Some(&first) = guard.front() {
                        if first == 0xE0 && guard.len() >= 2 && buffer.len() >= 2 {
                            let b0 = guard.pop_front().unwrap();
                            let b1 = guard.pop_front().unwrap();
                            drop(guard);
                            buffer[0] = b0;
                            buffer[1] = b1;
                            return 2;
                        }
                    }
                    drop(guard);
                }
                return copy_out(buffer, false);
            }
            // Not enough yet; block until new input arrives
            if self.nonblocking.load(Ordering::Relaxed) {
                return 0;
            }
            if let Some(task) = mytask() {
                self.input_waker.wait(task.get_id(), task.get_trapframe());
            } else {
                return 0;
            }
        }
    }
    fn read_byte(&self) -> Option<u8> {
        if self.is_current_process_group_background() {
            self.send_process_control_to_current_group(
                crate::ipc::event::ProcessControlType::TerminalInput,
            );
            return None;
        }

        // Loop until data becomes available
        loop {
            let mut input_buffer = self.input_buffer.lock();
            if let Some(byte) = input_buffer.pop_front() {
                return Some(byte);
            }
            drop(input_buffer);

            // No data available, block the current task
            if let Some(task) = mytask() {
                // Wait for input to become available
                // This will return when the task is woken up by input_waker.wake_all()
                self.input_waker.wait(task.get_id(), task.get_trapframe());

                // Continue the loop to re-check if data is available
                continue;
            } else {
                // No current task context, return None
                return None;
            }
        }
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        if self.is_tostop_enabled() && self.is_current_process_group_background() {
            self.send_process_control_to_current_group(
                crate::ipc::event::ProcessControlType::TerminalOutput,
            );
            return Err("Background TTY write stopped");
        }

        let _lock = self.write_lock.lock();

        if self.output_postprocess_enabled.load(Ordering::Relaxed) {
            let mut output = alloc::vec::Vec::with_capacity(buffer.len());
            for &byte in buffer {
                if byte == b'\n' {
                    output.push(b'\r');
                    output.push(b'\n');
                } else {
                    output.push(byte);
                }
            }
            self.backend.write(&output)?;
        } else {
            self.backend.write(buffer)?;
        }
        Ok(buffer.len())
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        if self.is_tostop_enabled() && self.is_current_process_group_background() {
            self.send_process_control_to_current_group(
                crate::ipc::event::ProcessControlType::TerminalOutput,
            );
            return Err("Background TTY write stopped");
        }

        let _lock = self.write_lock.lock();

        if self.output_postprocess_enabled.load(Ordering::Relaxed) && byte == b'\n' {
            self.backend.write(&[b'\r', b'\n'])?;
        } else {
            self.backend.write_byte(byte)?;
        }
        Ok(())
    }

    fn can_read(&self) -> bool {
        let input_buffer = self.input_buffer.lock();
        !input_buffer.is_empty()
    }

    fn can_write(&self) -> bool {
        self.backend.can_write()
    }
}

impl ControlOps for TtyDevice {
    // TTY devices accept Scarlet-private, OS-agnostic control opcodes.
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            SCTL_TTY_SET_ECHO => {
                self.set_echo(arg != 0);
                Ok(0)
            }
            SCTL_TTY_GET_ECHO => Ok(self.is_echo_enabled() as i32),
            SCTL_TTY_SET_CANONICAL => {
                self.set_canonical(arg != 0);
                Ok(0)
            }
            SCTL_TTY_GET_CANONICAL => Ok(self.is_canonical() as i32),
            SCTL_TTY_SET_SIGNAL_CHARS => {
                self.set_signal_chars_enabled(arg != 0);
                Ok(0)
            }
            SCTL_TTY_GET_SIGNAL_CHARS => Ok(self.is_signal_chars_enabled() as i32),
            SCTL_TTY_SET_CRNL_INPUT => {
                self.set_crnl_input_enabled(arg != 0);
                Ok(0)
            }
            SCTL_TTY_GET_CRNL_INPUT => Ok(self.is_crnl_input_enabled() as i32),
            SCTL_TTY_SET_OUTPUT_POSTPROCESS => {
                self.set_output_postprocess_enabled(arg != 0);
                Ok(0)
            }
            SCTL_TTY_GET_OUTPUT_POSTPROCESS => Ok(self.is_output_postprocess_enabled() as i32),
            SCTL_TTY_SET_EXTENDED_INPUT => {
                self.set_extended_input_enabled(arg != 0);
                Ok(0)
            }
            SCTL_TTY_GET_EXTENDED_INPUT => Ok(self.is_extended_input_enabled() as i32),
            SCTL_TTY_SET_WINSIZE => {
                let cols = ((arg >> 16) & 0xFFFF) as u16;
                let rows = (arg & 0xFFFF) as u16;
                self.set_winsize(cols, rows);
                Ok(0)
            }
            SCTL_TTY_GET_WINSIZE => {
                let (cols, rows) = self.get_winsize();
                let packed = ((cols as u32) << 16) | (rows as u32);
                Ok(packed as i32)
            }
            SCTL_TTY_SET_READ_POLICY => {
                let min_ready = (arg & 0xFFFF) as u16;
                let timeout_ms = ((arg >> 16) & 0xFFFF) as u16;
                self.read_min_ready_bytes
                    .store(min_ready, Ordering::Relaxed);
                self.read_timeout_ms.store(timeout_ms, Ordering::Relaxed);
                Ok(0)
            }
            SCTL_TTY_GET_READ_POLICY => {
                let min_ready = self.read_min_ready_bytes.load(Ordering::Relaxed) as u32;
                let timeout_ms = self.read_timeout_ms.load(Ordering::Relaxed) as u32;
                let packed = (timeout_ms << 16) | min_ready;
                Ok(packed as i32)
            }
            SCTL_TTY_FLUSH_INPUT => {
                let mut g = self.input_buffer.lock();
                g.clear();
                self.canonical_eof_ready.store(false, Ordering::Relaxed);
                self.literal_next_pending.store(false, Ordering::Relaxed);
                Ok(0)
            }
            SCTL_TTY_SET_DEBUG => {
                self.debug_enabled.store(arg != 0, Ordering::Relaxed);
                Ok(0)
            }
            SCTL_TTY_GET_DEBUG => Ok(self.debug_enabled.load(Ordering::Relaxed) as i32),
            SCTL_TTY_SET_KBMODE => {
                let v = (arg & 0xFF) as u8;
                self.kb_mode.store(v, Ordering::Relaxed);
                Ok(0)
            }
            SCTL_TTY_GET_KBMODE => Ok(self.kb_mode.load(Ordering::Relaxed) as i32),
            SCTL_TTY_SET_FOREGROUND_GROUP => {
                let task_group_id = self.resolve_foreground_task_group_id(arg);
                self.set_foreground_task_group_id(task_group_id);
                Ok(0)
            }
            SCTL_TTY_GET_FOREGROUND_GROUP => match self.get_foreground_task_group_id() {
                Some(id) => Ok(self.user_visible_foreground_task_group_id(id) as i32),
                None => Ok(-1),
            },
            SCTL_TTY_ACQUIRE_CONTROLLING => {
                self.acquire_as_controlling_tty(arg != 0)?;
                Ok(0)
            }
            SCTL_TTY_DETACH_CONTROLLING => {
                self.detach_controlling_tty()?;
                Ok(0)
            }
            _ => Err("Unsupported control command for TTY device"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tty() -> TtyDevice {
        let tty = TtyDevice::new("test_tty", 0);
        tty.set_echo(false);
        tty
    }

    #[test_case]
    fn test_tty_icrnl_maps_carriage_return_to_newline() {
        let tty = test_tty();
        tty.handle_input_byte(b'\r');

        let mut buffer = [0u8; 1];
        assert_eq!(tty.read(&mut buffer), 1);
        assert_eq!(buffer[0], b'\n');
    }

    #[test_case]
    fn test_tty_crnl_input_can_be_disabled() {
        let tty = test_tty();
        tty.set_canonical(false);
        tty.set_crnl_input_enabled(false);
        tty.handle_input_byte(b'\r');

        let mut buffer = [0u8; 1];
        assert_eq!(tty.read(&mut buffer), 1);
        assert_eq!(buffer[0], b'\r');
    }

    #[test_case]
    fn test_tty_signal_chars_can_be_disabled() {
        let tty = test_tty();
        tty.set_canonical(false);
        tty.set_signal_chars_enabled(false);
        tty.handle_input_byte(0x03);

        let mut buffer = [0u8; 1];
        assert_eq!(tty.read(&mut buffer), 1);
        assert_eq!(buffer[0], 0x03);
    }

    #[test_case]
    fn test_tty_configurable_erase_char() {
        let tty = test_tty();
        let mut chars = tty.get_control_chars();
        chars.erase = b'#';
        tty.set_control_chars(chars);

        tty.handle_input_byte(b'a');
        tty.handle_input_byte(b'#');
        tty.handle_input_byte(b'b');
        tty.handle_input_byte(b'\n');

        let mut buffer = [0u8; 2];
        assert_eq!(tty.read(&mut buffer), 2);
        assert_eq!(&buffer, b"b\n");
    }

    #[test_case]
    fn test_tty_canonical_eof_completes_pending_input() {
        let tty = test_tty();

        tty.handle_input_byte(b'a');
        tty.handle_input_byte(tty.get_control_chars().eof);

        let mut buffer = [0u8; 1];
        assert_eq!(tty.read(&mut buffer), 1);
        assert_eq!(buffer[0], b'a');
    }

    #[test_case]
    fn test_tty_literal_next_quotes_signal_char() {
        let tty = test_tty();
        let chars = tty.get_control_chars();

        tty.handle_input_byte(chars.literal_next);
        tty.handle_input_byte(chars.interrupt);
        tty.handle_input_byte(b'\n');

        let mut buffer = [0u8; 2];
        assert_eq!(tty.read(&mut buffer), 2);
        assert_eq!(buffer[0], chars.interrupt);
        assert_eq!(buffer[1], b'\n');
    }
}
