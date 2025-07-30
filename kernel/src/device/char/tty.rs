//! TTY (Terminal) device implementation.
//! 
//! This module implements a TTY device that acts as a terminal interface
//! providing line discipline, echo, and basic terminal I/O operations.

extern crate alloc;
use core::any::Any;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;
use crate::arch::get_cpu;
use crate::device::{Device, DeviceType};
use crate::device::char::CharDevice;
use crate::device::events::{DeviceEvent, DeviceEventListener, InputEvent, EventCapableDevice};
use crate::device::manager::DeviceManager;
use crate::drivers::uart;
use crate::sync::waker::Waker;
use crate::late_initcall;
use crate::task::mytask;
use crate::object::capability::{ControlOps, MemoryMappingOps};

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
    
    // Line discipline flags (Phase 2 expansion)
    canonical_mode: bool,
    echo_enabled: bool,
    
    // Terminal state (placeholder for future features)
    // process_group: Option<ProcessGroupId>,  // Job control placeholder
    // session_id: Option<SessionId>,           // Session management placeholder
}

impl TtyDevice {
    pub fn new(name: &'static str, uart_device_id: usize) -> Self {
        Self {
            name,
            uart_device_id,
            input_buffer: Arc::new(Mutex::new(VecDeque::new())),
            input_waker: Waker::new_interruptible("tty_input"),
            canonical_mode: true,
            echo_enabled: true,
        }
    }
    
    /// Handle input byte from UART device.
    /// 
    /// This method processes incoming bytes and applies line discipline.
    fn handle_input_byte(&self, byte: u8) {
        // crate::early_println!("TTY processing byte: {:02x}", byte);
        
        // Phase 2: Canonical mode processing
        if self.canonical_mode {
            match byte {
                // Backspace/DEL
                0x08 | 0x7F => {
                    // crate::early_println!("TTY: Backspace detected");
                    let mut input_buffer = self.input_buffer.lock();
                    if input_buffer.pop_back().is_some() && self.echo_enabled {
                        self.echo_backspace();
                    }
                }
                // Enter/Line feed
                b'\r' | b'\n' => {
                    // crate::early_println!("TTY: Enter/newline detected");
                    if self.echo_enabled {
                        self.echo_char(b'\r');
                        self.echo_char(b'\n');
                    }
                    let mut input_buffer = self.input_buffer.lock();
                    input_buffer.push_back(b'\n');
                    // crate::early_println!("TTY: Line added to buffer, size now: {}", input_buffer.len());
                    // Wake up waiting processes
                    drop(input_buffer);
                    self.input_waker.wake_all();
                }
                // Control characters (placeholder for signal processing)
                0x03 => {
                    crate::early_println!("TTY: Ctrl+C detected");
                    // Ctrl+C: Send SIGINT (placeholder)
                    // TODO: Implement signal processing when process management is ready
                }
                0x1A => {
                    crate::early_println!("TTY: Ctrl+Z detected");
                    // Ctrl+Z: Send SIGTSTP (placeholder)
                    // TODO: Implement job control when process management is ready
                }
                // Regular characters
                byte => {
                    // crate::early_println!("TTY: Regular character: {:02x}", byte);
                    if self.echo_enabled {
                        self.echo_char(byte);
                    }
                    let mut input_buffer = self.input_buffer.lock();
                    input_buffer.push_back(byte);
                    // crate::early_println!("TTY: Character added to buffer, size now: {}", input_buffer.len());
                    // Wake up waiting processes in RAW mode or for immediate input
                    drop(input_buffer);
                    if !self.canonical_mode {
                        self.input_waker.wake_all();
                    }
                }
            }
        } else {
            // RAW mode: Pass through directly
            let mut input_buffer = self.input_buffer.lock();
            input_buffer.push_back(byte);
            drop(input_buffer);
            // Wake up waiting processes immediately in RAW mode
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

impl DeviceEventListener for TtyDevice {
    fn on_device_event(&self, event: &dyn DeviceEvent) {
        if let Some(input_event) = event.as_any().downcast_ref::<InputEvent>() {
            // crate::early_println!("TTY received input event: byte={:02x} ('{}')", 
            //     input_event.data, 
            //     if input_event.data.is_ascii_graphic() || input_event.data == b' ' { 
            //         input_event.data as char 
            //     } else { 
            //         '?' 
            //     });
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
}

impl CharDevice for TtyDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut input_buffer = self.input_buffer.lock();
        if let Some(byte) = input_buffer.pop_front() {
            return Some(byte);
        }
        drop(input_buffer);
        
        // No data available, block the current task
        if let Some(task) = mytask() {
            let mut cpu = get_cpu();

            // This never returns - the syscall will be restarted when the task is woken up
            self.input_waker.wait(task.get_id(), &mut cpu);
        }

        None
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
    // TTY devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}
