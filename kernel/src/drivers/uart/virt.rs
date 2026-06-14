// UART driver for QEMU virt machine

use alloc::{boxed::Box, collections::VecDeque, sync::Arc};
use core::any::Any;
use spin::{Mutex, RwLock};

use crate::{
    device::{
        Device, DeviceInfo, DeviceType,
        char::CharDevice,
        events::{
            DeviceEventEmitter, DeviceEventListener, EventCapableDevice, InputEvent,
            InterruptCapableDevice,
        },
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    driver_initcall,
    interrupt::InterruptId,
    object::capability::{ControlOps, MemoryMappingOps, Selectable},
};

const TX_PACE_BYTES: usize = 64;

pub struct Uart {
    // inner: Arc<Mutex<UartInner>>,
    base: usize,
    interrupt_id: RwLock<Option<InterruptId>>,
    rx_buffer: Mutex<VecDeque<u8>>,
    event_emitter: Mutex<DeviceEventEmitter>,
    // Serializes TX access across all callers (kernel _print, TTY write, echo).
    tx_lock: Mutex<()>,
}

pub const RHR_OFFSET: usize = 0x00;
pub const THR_OFFSET: usize = 0x00;
pub const IER_OFFSET: usize = 0x01; // Interrupt Enable Register
pub const IIR_OFFSET: usize = 0x02; // Interrupt Identification Register
pub const FCR_OFFSET: usize = 0x02; // FIFO Control Register (write only)
pub const LCR_OFFSET: usize = 0x03; // Line Control Register
pub const LSR_OFFSET: usize = 0x05;

pub const LSR_THRE: u8 = 0x20;
pub const LSR_TEMT: u8 = 0x40;
pub const LSR_DR: u8 = 0x01;

// IER bits
pub const IER_RDA: u8 = 0x01; // Received Data Available
pub const IER_RLS: u8 = 0x04; // Receiver Line Status

// IIR bits
pub const IIR_PENDING: u8 = 0x01; // 0=interrupt pending, 1=no interrupt
pub const IIR_RDA: u8 = 0x04; // Received Data Available

// FCR bits
pub const FCR_ENABLE: u8 = 0x01; // FIFO enable
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
            tx_lock: Mutex::new(()),
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

    /// Enable UART-side interrupts after the controller line has been registered.
    pub fn enable_interrupts(&self, interrupt_id: InterruptId) -> Result<(), &'static str> {
        self.interrupt_id.write().replace(interrupt_id);
        // Enable receive data available interrupt
        self.reg_write(IER_OFFSET, IER_RDA);

        Ok(())
    }

    fn reg_write(&self, offset: usize, value: u8) {
        let addr = self.base + offset;
        unsafe { crate::arch::mmio::write8(addr, value) }
    }

    fn reg_read(&self, offset: usize) -> u8 {
        let addr = self.base + offset;
        unsafe { crate::arch::mmio::read8(addr) }
    }

    fn write_byte_internal(&self, c: u8) {
        while self.reg_read(LSR_OFFSET) & LSR_THRE == 0 {
            core::hint::spin_loop();
        }
        self.reg_write(THR_OFFSET, c);
    }

    fn wait_tx_idle(&self) {
        while self.reg_read(LSR_OFFSET) & LSR_TEMT == 0 {
            core::hint::spin_loop();
        }
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

    fn drain_rx(&self) {
        // Drain all available RX bytes. Reading only one byte can leave the
        // FIFO non-empty without producing a new edge, which loses interactive
        // input such as "ls\n" after the first interrupt.
        while self.can_read() {
            let c = self.read_byte_internal();
            self.emit_event(&InputEvent { data: c });
        }
    }
}

impl MemoryMappingOps for Uart {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for UART")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // UART devices don't support memory mapping
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // UART devices don't support memory mapping
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

static UART_CAPS: [crate::device::DeviceCapability; 1] = [crate::device::DeviceCapability::Serial];

impl Device for Uart {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "virt-uart"
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

    fn capabilities(&self) -> &'static [crate::device::DeviceCapability] {
        &UART_CAPS
    }

    fn as_event_capable(&self) -> Option<&dyn EventCapableDevice> {
        Some(self)
    }
}

impl CharDevice for Uart {
    fn read_byte(&self) -> Option<u8> {
        let mut buffer = self.rx_buffer.lock();
        buffer.pop_front()
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        let _lock = self.tx_lock.lock();

        self.write_byte_internal(byte);
        if byte == b'\n' {
            self.wait_tx_idle();
        }
        Ok(())
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let _lock = self.tx_lock.lock();

        let mut paced = 0;
        for &byte in buffer {
            self.write_byte_internal(byte);
            paced += 1;
            if byte == b'\n' || paced >= TX_PACE_BYTES {
                self.wait_tx_idle();
                paced = 0;
            }
        }

        Ok(buffer.len())
    }

    fn can_read(&self) -> bool {
        self.can_read()
    }

    fn can_write(&self) -> bool {
        self.can_write()
    }
}

impl ControlOps for Uart {
    // UART devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
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
        let iir = self.reg_read(IIR_OFFSET);
        if iir & IIR_PENDING == 0 {
            let cause = iir & 0x0E;
            if cause == IIR_RDA {
                self.drain_rx();
            }
        }
        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        self.interrupt_id.read().clone()
    }
}

impl Selectable for Uart {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

fn register_uart() {
    use alloc::vec;

    // Create UART platform device driver
    let driver = Box::new(PlatformDeviceDriver::new(
        "virt-uart-driver",
        uart_probe,
        uart_remove,
        vec!["ns16550a", "ns16550", "uart16550", "serial"],
    ));

    // Register with Core priority since UART is essential for early console output
    DeviceManager::get_manager().register_driver(driver, DriverPriority::Core);
}

/// Probe function for UART devices
fn uart_probe(device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    crate::early_println!("Probing UART device: {}", device_info.name());

    // Get memory resource (base address)
    let memory_resource = device_info
        .get_resources()
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("No memory resource found for UART")?;

    let paddr = memory_resource.start;
    let size = memory_resource.end - memory_resource.start + 1;
    crate::early_println!("UART paddr: {:#x}, size: {:#x}", paddr, size);

    // Map the UART's physical MMIO region into the kernel virtual address space.
    let base_addr = crate::vm::ioremap(paddr, size).map_err(|e| {
        crate::early_println!("UART ioremap({:#x}, {:#x}) failed: {}", paddr, size, e);
        e
    })?;
    crate::early_println!("UART base address (virt): {:#x}", base_addr);

    // Create UART instance
    let uart = Arc::new(Uart::new(base_addr));

    // Initialize UART
    uart.init();

    // Get interrupt resource if available
    if let Some(irq_resource) = device_info
        .get_resources()
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::IRQ)
    {
        let uart_interrupt_id = crate::interrupt::register_and_enable_platform_irq_device(
            irq_resource,
            uart.clone(),
            crate::arch::get_cpu().get_cpuid() as u32,
        )
        .map_err(|_| "Failed to register UART interrupt")?;
        crate::early_println!("UART interrupt ID: {}", uart_interrupt_id);

        // Enable UART interrupts
        if let Err(e) = uart.enable_interrupts(uart_interrupt_id) {
            crate::early_println!("Failed to enable UART interrupts: {}", e);
            // Continue without interrupts - polling mode will work
        } else {
            crate::early_println!("UART interrupts enabled (ID: {})", uart_interrupt_id);
            crate::early_println!("UART interrupt device registered");
        }
    } else {
        crate::early_println!("No interrupt resource found for UART, using polling mode");
    }

    // Register the UART device with the device manager
    let device_id = DeviceManager::get_manager().register_device(uart);
    crate::early_println!("UART device registered with ID: {}", device_id);

    Ok(())
}

/// Remove function for UART devices  
fn uart_remove(_device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    // TODO: Implement device removal logic
    Ok(())
}

driver_initcall!(register_uart);
