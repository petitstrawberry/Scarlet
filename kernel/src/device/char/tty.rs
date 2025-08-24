//! TTY (Terminal) device implementation.
//! 
//! This module implements a TTY device that acts as a terminal interface
//! providing line discipline, echo, and basic terminal I/O operations.

extern crate alloc;
use core::any::Any;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::arch::get_cpu;
use crate::device::{Device, DeviceType, DeviceCapability};
use crate::device::char::{CharDevice, TtyControl};
use crate::device::events::{DeviceEvent, DeviceEventListener, InputEvent, EventCapableDevice};
use crate::device::manager::DeviceManager;
use crate::sync::waker::Waker;
use crate::late_initcall;
use crate::task::mytask;
use crate::object::capability::{ControlOps, MemoryMappingOps};

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
}
use tty_ctl::*;

// Provide a static capabilities slice for TTY devices
static TTY_CAPS: [DeviceCapability; 1] = [DeviceCapability::Tty];

/// TTY subsystem initialization
fn init_tty_subsystem() {
    let result = try_init_tty_subsystem();
    if let Err(e) = result {
        crate::early_println!("Failed to initialize TTY subsystem: {}", e);
    }
}

fn try_init_tty_subsystem() -> Result<(), &'static str> {
    let device_manager = DeviceManager::get_manager();
    
    // Find the first UART device and use its ID for TTY initialization
    if let Some(uart_device_id) = device_manager.get_first_device_by_type(crate::device::DeviceType::Char) {
        // Create TTY device with UART device ID for lookup
        let tty_device = Arc::new(TtyDevice::new("tty0", uart_device_id));
        let uart_device = device_manager.get_device(uart_device_id).ok_or("UART device not found")?;
        
        // Register TTY device as event listener for UART
        if let Some(uart) = uart_device.as_any().downcast_ref::<crate::drivers::uart::virt::Uart>() {
            let weak_tty = Arc::downgrade(&tty_device);
            // Register TTY as event listener for UART input events
            uart.register_event_listener(weak_tty);
            crate::early_println!("TTY registered as UART event listener");
        } else {
            crate::early_println!("Failed to cast UART device to specific type");
        }
        
        // Register TTY device with device manager
        let _tty_id = device_manager.register_device_with_name("tty0".into(), tty_device);
        
        crate::early_println!("TTY subsystem initialized successfully");
        Ok(())
    } else {
        Err("No UART device found for TTY initialization")
    }
}

late_initcall!(init_tty_subsystem);

/// TTY device implementation.
/// 
/// This device provides terminal functionality including line discipline,
/// echo, and basic terminal I/O operations.
pub struct TtyDevice {
    name: &'static str,
    uart_device_id: usize,
    
    // Input buffer for line discipline
    input_buffer: Arc<Mutex<VecDeque<u8>>>,
    
    // Waker for blocking reads
    input_waker: Waker,
    
    // Line discipline flags (OS/ABI-neutral)
    canonical_mode: AtomicBool,
    echo_enabled: AtomicBool,

    // Window size in character cells (OS/ABI-neutral)
    winsize_cols: Mutex<u16>,
    winsize_rows: Mutex<u16>,
}

impl TtyDevice {
    pub fn new(name: &'static str, uart_device_id: usize) -> Self {
        Self {
            name,
            uart_device_id,
            input_buffer: Arc::new(Mutex::new(VecDeque::new())),
            input_waker: Waker::new_interruptible("tty_input"),
            canonical_mode: AtomicBool::new(true),
            echo_enabled: AtomicBool::new(true),
            winsize_cols: Mutex::new(80),
            winsize_rows: Mutex::new(25),
        }
    }
    
    /// Handle input byte from UART device.
    /// 
    /// This method processes incoming bytes and applies line discipline.
    fn handle_input_byte(&self, byte: u8) {
        // Canonical mode processing
        if self.canonical_mode.load(Ordering::Relaxed) {
            match byte {
                // Backspace/DEL
                0x08 | 0x7F => {
                    let mut input_buffer = self.input_buffer.lock();
                    if input_buffer.pop_back().is_some() && self.echo_enabled.load(Ordering::Relaxed) {
                        self.echo_backspace();
                    }
                }
                // Enter/Line feed
                b'\r' | b'\n' => {
                    if self.echo_enabled.load(Ordering::Relaxed) {
                        self.echo_char(b'\r');
                        self.echo_char(b'\n');
                    }
                    let mut input_buffer = self.input_buffer.lock();
                    input_buffer.push_back(b'\n');
                    drop(input_buffer);
                    self.input_waker.wake_all();
                }
                // Ctrl-C (ETX) — policy deferred to ABI layer; no signal delivery here
                0x03 => {
                    if self.echo_enabled.load(Ordering::Relaxed) {
                        self.echo_char('^' as u8);
                        self.echo_char('C' as u8);
                        self.echo_char('\r' as u8);
                        self.echo_char('\n' as u8);
                    }
                }
                // Ctrl-Z (SUB) placeholder
                0x1A => {
                    // No job-control semantics in device layer
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
            // RAW mode: Pass through directly
            let mut input_buffer = self.input_buffer.lock();
            input_buffer.push_back(byte);
            drop(input_buffer);
            self.input_waker.wake_all();
        }
    }
    
    /// Echo character back to output.
    fn echo_char(&self, byte: u8) {
        // Get actual UART device and output
        let device_manager = DeviceManager::get_manager();
        if let Some(uart_device) = device_manager.get_device(self.uart_device_id) {
            // Use the new CharDevice API with internal mutability
            if let Some(char_device) = uart_device.as_char_device() {
                let _ = char_device.write_byte(byte);
            }
        }
    }
    
    /// Echo backspace sequence.
    fn echo_backspace(&self) {
        // Backspace echo: BS + space + BS
        self.echo_char(0x08);
        self.echo_char(b' ');
        self.echo_char(0x08);
    }
}

impl TtyControl for TtyDevice {
    fn set_echo(&self, enabled: bool) {
        self.echo_enabled.store(enabled, Ordering::Relaxed);
    }
    fn is_echo_enabled(&self) -> bool { self.echo_enabled.load(Ordering::Relaxed) }

    fn set_canonical(&self, enabled: bool) {
        self.canonical_mode.store(enabled, Ordering::Relaxed);
    }
    fn is_canonical(&self) -> bool { self.canonical_mode.load(Ordering::Relaxed) }

    fn set_winsize(&self, cols: u16, rows: u16) {
        *self.winsize_cols.lock() = cols;
        *self.winsize_rows.lock() = rows;
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
    fn get_mapping_info(&self, _offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
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
    fn read_byte(&self) -> Option<u8> {
        // Loop until data becomes available
        loop {
            let mut input_buffer = self.input_buffer.lock();
            if let Some(byte) = input_buffer.pop_front() {
                return Some(byte);
            }
            drop(input_buffer);
            
            // No data available, block the current task
            if let Some(mut task) = mytask() {
                let mut cpu = get_cpu();

                // Wait for input to become available
                // This will return when the task is woken up by input_waker.wake_all()
                self.input_waker.wait(task.get_id(), &mut cpu);
                
                // Continue the loop to re-check if data is available
                continue;
            } else {
                // No current task context, return None
                return None;
            }
        }
    }
    
    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        // Forward to UART device with line ending conversion
        let device_manager = DeviceManager::get_manager();
        if let Some(uart_device) = device_manager.get_device(self.uart_device_id) {
            // Use the new CharDevice API with internal mutability
            if let Some(char_device) = uart_device.as_char_device() {
                // Handle line ending conversion for terminals
                if byte == b'\n' {
                    char_device.write_byte(b'\r')?;
                    char_device.write_byte(b'\n')?;
                } else {
                    char_device.write_byte(byte)?;
                }
                return Ok(());
            }
        }
        Err("UART device not available")
    }
    
    fn can_read(&self) -> bool {
        let input_buffer = self.input_buffer.lock();
        !input_buffer.is_empty()
    }
    
    fn can_write(&self) -> bool {
        // Check if UART device is available
        let device_manager = DeviceManager::get_manager();
        if let Some(uart_device) = device_manager.get_device(self.uart_device_id) {
            if let Some(uart) = uart_device.as_any().downcast_ref::<crate::drivers::uart::virt::Uart>() {
                return uart.can_write();
            }
        }
        false
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
            SCTL_TTY_GET_ECHO => {
                Ok(self.is_echo_enabled() as i32)
            }
            SCTL_TTY_SET_CANONICAL => {
                self.set_canonical(arg != 0);
                Ok(0)
            }
            SCTL_TTY_GET_CANONICAL => {
                Ok(self.is_canonical() as i32)
            }
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
            _ => Err("Unsupported control command for TTY device"),
        }
    }
}
