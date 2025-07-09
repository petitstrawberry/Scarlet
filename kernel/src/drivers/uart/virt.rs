// UART driver for QEMU virt machine

use core::{fmt, any::Any, ptr::{read_volatile, write_volatile}};
use core::fmt::Write;
use alloc::{boxed::Box, collections::VecDeque, sync::Arc};
use spin::{Mutex, RwLock};

use crate::{
    device::{
        char::CharDevice, events::{DeviceEventEmitter, DeviceEventListener, EventCapableDevice, InputEvent, InterruptCapableDevice}, manager::{DeviceManager, DriverPriority}, platform::{
            resource::PlatformDeviceResourceType, PlatformDeviceDriver, PlatformDeviceInfo
        }, Device, DeviceInfo, DeviceType
    }, driver_initcall, drivers::uart, interrupt::{InterruptId, InterruptManager}, traits::serial::Serial
};

pub struct Uart {
    // inner: Arc<Mutex<UartInner>>,
    base: usize,
    interrupt_id: RwLock<Option<InterruptId>>,
    rx_buffer: Mutex<VecDeque<u8>>,
    event_emitter: Mutex<DeviceEventEmitter>,
}

pub const RHR_OFFSET: usize = 0x00;
pub const THR_OFFSET: usize = 0x00;
pub const IER_OFFSET: usize = 0x01;  // Interrupt Enable Register
pub const IIR_OFFSET: usize = 0x02;  // Interrupt Identification Register
pub const FCR_OFFSET: usize = 0x02;  // FIFO Control Register (write only)
pub const LCR_OFFSET: usize = 0x03;  // Line Control Register
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

pub const LCR_BAUD_LATCH: u8 = 0x80; // Set baud rate divisor latch access bit

impl Uart {
    pub fn new(base: usize) -> Self {
        Uart { 
            base,
            interrupt_id: RwLock::new(None),
            rx_buffer: Mutex::new(VecDeque::new()),
            event_emitter: Mutex::new(DeviceEventEmitter::new()),
        }
    }

    pub fn init(&self) {
        // Disable all interrupts
        self.reg_write(IER_OFFSET, 0x00);

        // Set special mode to set baud rate
        self.reg_write(LCR_OFFSET, LCR_BAUD_LATCH);

        // LSB of baud rate divisor
        self.reg_write(0x00, 0x03);

        // MSB of baud rate divisor
        self.reg_write(0x01, 0x00);

        // Set line control register for 8 data bits, no parity, 1 stop bit
        self.reg_write(LCR_OFFSET, 0x03); // 8 bits, no

        // Enable FIFO
        self.reg_write(FCR_OFFSET, FCR_ENABLE | FCR_CLEAR_RX | FCR_CLEAR_TX);
    }

    /// Enable UART interrupts
    pub fn enable_interrupts(&self, interrupt_id: InterruptId) -> Result<(), &'static str> {
        self.interrupt_id.write().replace(interrupt_id);
        // Enable receive data available interrupt
        self.reg_write(IER_OFFSET, IER_RDA);

        // Register interrupt with interrupt manager
        InterruptManager::with_manager(|mgr| {
            mgr.enable_external_interrupt(interrupt_id, 0) // Enable for CPU 0
        }).map_err(|_| "Failed to enable interrupt")?;
        
        Ok(())
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

    fn can_read(&self) -> bool {
        self.reg_read(LSR_OFFSET) & LSR_DR != 0
    }

    fn can_write(&self) -> bool {
        self.reg_read(LSR_OFFSET) & LSR_THRE != 0
    }
}

impl Serial for Uart {
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
    fn put(&self, c: char) -> fmt::Result {
        self.write_byte_internal(c as u8); // Block until ready
        Ok(())
    }

    /// Reads a character from the UART. (non-blocking)
    /// 
    /// Returns `Some(char)` if a character is available, or `None` if not.
    /// If interrupts are enabled, reads from the interrupt buffer.
    /// Otherwise, falls back to polling mode.
    /// 
    fn get(&self) -> Option<char> {
        let mut buffer = self.rx_buffer.lock();
            // Try to read from interrupt buffer
        if let Some(byte) = buffer.pop_front() {
            return Some(byte as char);
        }

        None
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
    
    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for Uart {
    fn read_byte(&self) -> Option<u8> {
        let mut buffer = self.rx_buffer.lock();
            // Try to read from interrupt buffer
        if let Some(byte) = buffer.pop_front() {
            return Some(byte);
        }

        None
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.write_byte_internal(byte); // Block until ready
        Ok(())
    }

    fn can_read(&self) -> bool {
        self.can_read()
    }

    fn can_write(&self) -> bool {
        self.can_write()
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

impl EventCapableDevice for Uart {
    fn register_event_listener(&self, listener: alloc::sync::Weak<dyn DeviceEventListener>) {
        self.event_emitter.lock().register_listener(listener);
    }
    
    fn unregister_event_listener(&self, _listener_id: &str) {
        // Implementation later - normally WeakRef is automatically removed
    }
    
    fn emit_event(&self, event: &dyn crate::device::events::DeviceEvent) {
        self.event_emitter.lock().emit(event);
    }
}

impl InterruptCapableDevice for Uart {
    fn handle_interrupt(&self) -> crate::interrupt::InterruptResult<()> {
        // let inner = self.inner.lock();
        // Check interrupt identification register
        let iir = self.reg_read(IIR_OFFSET);
        
        if iir & IIR_PENDING == 0 {
            let c = self.read_byte_internal();
            if c != 0 {
                // Emit received character event
                self.emit_event(
                    &InputEvent {
                        data: c as u8,
                    }
                );
            } else {
                // No data available, return Ok
                return Ok(());
            }
        }
        
        Ok(())
    }
    
    fn interrupt_id(&self) -> Option<InterruptId> {
        self.interrupt_id.read().clone()
    }
}

// impl UartInner {
//     fn reg_write(&self, offset: usize, value: u8) {
//         let addr = self.base + offset;
//         unsafe { write_volatile(addr as *mut u8, value) }
//     }

//     fn reg_read(&self, offset: usize) -> u8 {
//         let addr = self.base + offset;
//         unsafe { read_volatile(addr as *const u8) }
//     }

//     fn write_byte_internal(&self, c: u8) {
//         while self.reg_read(LSR_OFFSET) & LSR_THRE == 0 {}
//         self.reg_write(THR_OFFSET, c);
//     }

//     fn read_byte_internal(&self) -> u8 {
//         if self.reg_read(LSR_OFFSET) & LSR_DR == 0 {
//             return 0;
//         }
//         self.reg_read(RHR_OFFSET)
//     }

//     fn can_read(&self) -> bool {
//         self.reg_read(LSR_OFFSET) & LSR_DR != 0
//     }

//     fn can_write(&self) -> bool {
//         self.reg_read(LSR_OFFSET) & LSR_THRE != 0
//     }
// }

fn register_uart() {
    use alloc::vec;
    
    // Create UART platform device driver
    let driver = Box::new(PlatformDeviceDriver::new(
        "virt-uart-driver",
        uart_probe,
        uart_remove,
        vec!["ns16550a", "ns16550", "uart16550", "serial"]
    ));
    
    // Register with Core priority since UART is essential for early console output
    DeviceManager::get_mut_manager().register_driver(driver, DriverPriority::Core);
}

/// Probe function for UART devices
fn uart_probe(device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    crate::early_println!("Probing UART device: {}", device_info.name());
    
    // Get memory resource (base address)
    let memory_resource = device_info.get_resources()
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("No memory resource found for UART")?;
    
    let base_addr = memory_resource.start;
    crate::early_println!("UART base address: 0x{:x}", base_addr);
    
    // Create UART instance
    let uart = Arc::new(Uart::new(base_addr));

    // Initialize UART
    uart.init();

    // Get interrupt resource if available
    if let Some(irq_resource) = device_info.get_resources()
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::IRQ) {
        
        let uart_interrupt_id = irq_resource.start as u32;
        crate::early_println!("UART interrupt ID: {}", uart_interrupt_id);
        
        // Enable UART interrupts
        if let Err(e) = uart.enable_interrupts(uart_interrupt_id) {
            crate::early_println!("Failed to enable UART interrupts: {}", e);
            // Continue without interrupts - polling mode will work
        } else {
            crate::early_println!("UART interrupts enabled (ID: {})", uart_interrupt_id);
            
            // Register interrupt handler
            if let Err(e) = InterruptManager::with_manager(|mgr| {
                mgr.register_interrupt_device(uart_interrupt_id, uart.clone())
            }) {
                crate::early_println!("Failed to register UART interrupt device: {}", e);
            } else {
                crate::early_println!("UART interrupt device registered using trait-based system");
            }
        }
    } else {
        crate::early_println!("No interrupt resource found for UART, using polling mode");
    }
    
    // Register the UART device with the device manager
    let device_id = DeviceManager::get_mut_manager().register_device(uart);
    crate::early_println!("UART device registered with ID: {}", device_id);

    Ok(())
}

/// Remove function for UART devices  
fn uart_remove(_device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    // TODO: Implement device removal logic
    Ok(())
}

driver_initcall!(register_uart);