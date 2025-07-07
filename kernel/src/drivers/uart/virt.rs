// UART driver for QEMU virt machine

use core::{fmt, any::Any, ptr::{read_volatile, write_volatile}};
use core::fmt::Write;
use alloc::{boxed::Box, collections::VecDeque};
use spin::Mutex;

use crate::{device::{char::CharDevice, Device, DeviceType}, driver_initcall, early_initcall, interrupt::{InterruptId, InterruptManager}, traits::serial::Serial};

#[derive(Clone)]
pub struct Uart {
    base: usize,
    interrupt_id: Option<InterruptId>,
    rx_buffer: Option<alloc::sync::Arc<Mutex<VecDeque<u8>>>>,
}

pub const RHR_OFFSET: usize = 0x00;
pub const THR_OFFSET: usize = 0x00;
pub const IER_OFFSET: usize = 0x01;  // Interrupt Enable Register
pub const IIR_OFFSET: usize = 0x02;  // Interrupt Identification Register
pub const FCR_OFFSET: usize = 0x02;  // FIFO Control Register (write only)
pub const LSR_OFFSET: usize = 0x05;

pub const LSR_THRE: u8 = 0x20;
pub const LSR_DR: u8 = 0x01;

// IER bits
pub const IER_RDA: u8 = 0x01;    // Received Data Available
pub const IER_THRE: u8 = 0x02;   // Transmit Holding Register Empty
pub const IER_RLS: u8 = 0x04;    // Receiver Line Status

// IIR bits
pub const IIR_PENDING: u8 = 0x01; // 0=interrupt pending, 1=no interrupt
pub const IIR_RDA: u8 = 0x04;     // Received Data Available
pub const IIR_THRE: u8 = 0x02;    // Transmit Holding Register Empty

// FCR bits
pub const FCR_ENABLE: u8 = 0x01;   // FIFO enable
pub const FCR_CLEAR_RX: u8 = 0x02; // Clear receive FIFO
pub const FCR_CLEAR_TX: u8 = 0x04; // Clear transmit FIFO

impl Uart {
    pub fn new(base: usize) -> Self {
        Uart { 
            base,
            interrupt_id: None,
            rx_buffer: None,
        }
    }

    fn reg_write(&self, offset: usize, value: u8) {
        let addr = self.base + offset;
        unsafe { write_volatile(addr as *mut u8, value) }
    }

    fn reg_read(&self, offset: usize) -> u8 {
        let addr = self.base + offset;
        unsafe { read_volatile(addr as *const u8) }
    }

    fn write_byte_internal(&self, c: u8) {
        while self.reg_read(LSR_OFFSET) & LSR_THRE == 0 {}
        self.reg_write(THR_OFFSET, c);
    }

    fn read_byte_internal(&self) -> u8 {
        if self.reg_read(LSR_OFFSET) & LSR_DR == 0 {
            return 0;
        }
        self.reg_read(RHR_OFFSET)
    }

    /// Enable UART interrupts
    pub fn enable_interrupts(&mut self, interrupt_id: InterruptId) -> Result<(), &'static str> {
        self.interrupt_id = Some(interrupt_id);
        
        // Create shared receive buffer
        self.rx_buffer = Some(alloc::sync::Arc::new(Mutex::new(VecDeque::new())));
        
        // Enable FIFO
        self.reg_write(FCR_OFFSET, FCR_ENABLE | FCR_CLEAR_RX | FCR_CLEAR_TX);
        
        // Enable receive data available interrupt
        self.reg_write(IER_OFFSET, IER_RDA);
        
        // Register interrupt with interrupt manager
        InterruptManager::with_manager(|mgr| {
            mgr.enable_external_interrupt(interrupt_id, 0) // Enable for CPU 0
        }).map_err(|_| "Failed to enable interrupt")?;
        
        Ok(())
    }

    /// Get the receive buffer (used by interrupt handler)
    pub fn get_rx_buffer(&self) -> Option<alloc::sync::Arc<Mutex<VecDeque<u8>>>> {
        self.rx_buffer.clone()
    }
}

impl Serial for Uart {
    fn init(&mut self) {
        // Initialization code for the UART can be added here if needed.
        // For now, we assume the UART is already initialized by the QEMU virt machine.
    }

    /// Writes a character to the UART. (blocking)
    /// 
    /// This function will block until the UART is ready to accept the character.
    /// 
    /// # Arguments
    /// * `c` - The character to write to the UART
    /// 
    /// # Returns
    /// A `fmt::Result` indicating success or failure.
    /// 
    fn put(&mut self, c: char) -> fmt::Result {
        self.write_byte_internal(c as u8); // Block until ready
        Ok(())
    }

    /// Reads a character from the UART. (non-blocking)
    /// 
    /// Returns `Some(char)` if a character is available, or `None` if not.
    /// If interrupts are enabled, reads from the interrupt buffer.
    /// Otherwise, falls back to polling mode.
    /// 
    fn get(&mut self) -> Option<char> {
        // Try to read from interrupt buffer first
        if let Some(buffer) = &self.rx_buffer {
            if let Some(byte) = buffer.lock().pop_front() {
                return Some(byte as char);
            }
        }
        
        // Fallback to polling mode
        if self.can_read() {
            Some(self.read_byte_internal() as char)
        } else {
            None
        }
    }

    /// Get a mutable reference to Any for downcasting
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Device for Uart {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "virt-uart"
    }

    fn id(&self) -> usize {
        0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    
    fn as_char_device(&mut self) -> Option<&mut dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for Uart {
    fn read_byte(&mut self) -> Option<u8> {
        // Try to read from interrupt buffer first
        if let Some(buffer) = &self.rx_buffer {
            if let Some(byte) = buffer.lock().pop_front() {
                return Some(byte);
            }
        }
        
        // Fallback to polling mode
        if self.can_read() {
            Some(self.read_byte_internal())
        } else {
            None
        }
    }

    fn write_byte(&mut self, byte: u8) -> Result<(), &'static str> {
        self.write_byte_internal(byte); // Block until ready
        Ok(())
    }

    fn can_read(&self) -> bool {
        self.reg_read(LSR_OFFSET) & LSR_DR != 0
    }

    fn can_write(&self) -> bool {
        self.reg_read(LSR_OFFSET) & LSR_THRE != 0
    }
    
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            if c == '\n' {
                self.put('\r')?; // Convert newline to carriage return + newline
            }
            self.put(c)?;
        }
        Ok(())
    }
}

/// UART interrupt handler
fn uart_interrupt_handler(handle: &mut crate::interrupt::InterruptHandle) -> crate::interrupt::InterruptResult<()> {
    // Get UART device from device manager
    let device_manager = crate::device::manager::DeviceManager::get_mut_manager();
    if let Some(serial) = device_manager.basic.borrow_mut_serial(0) {
        // Cast to Uart to access interrupt-specific methods
        if let Some(uart) = serial.as_any_mut().downcast_mut::<Uart>() {
            // Check interrupt identification register
            let iir = uart.reg_read(IIR_OFFSET);
            
            if iir & IIR_PENDING == 0 { // Interrupt pending
                match iir & 0x0E { // Interrupt type
                    IIR_RDA => {
                        // Received Data Available interrupt
                        if let Some(buffer) = uart.get_rx_buffer() {
                            while uart.can_read() {
                                let byte = uart.read_byte_internal();
                                buffer.lock().push_back(byte);
                            }
                        }
                    }
                    IIR_THRE => {
                        // Transmit Holding Register Empty interrupt
                        // TODO: Handle transmit interrupt if needed
                    }
                    _ => {
                        // Other interrupt types
                    }
                }
            }
        }
    }
    
    // Complete the interrupt
    handle.complete()
}

fn register_uart() {
    let mut uart = Uart::new(0x1000_0000);
    
    // TODO: get id from fdt
    let uart_interrupt_id = 10;
    
    // 割り込みを有効化
    if let Err(e) = uart.enable_interrupts(uart_interrupt_id) {
        crate::early_println!("Failed to enable UART interrupts: {}", e);
        // If enabling interrupts fails, we can still use UART in polling mode
    } else {
        crate::early_println!("UART interrupts enabled (ID: {})", uart_interrupt_id);
        
        // Register interrupt handler
        if let Err(e) = InterruptManager::with_manager(|mgr| {
            mgr.register_external_handler(uart_interrupt_id, uart_interrupt_handler)
        }) {
            crate::early_println!("Failed to register UART interrupt handler: {}", e);
        } else {
            crate::early_println!("UART interrupt handler registered");
        }
    }
    
    crate::device::manager::register_serial(Box::new(uart));
}

driver_initcall!(register_uart);