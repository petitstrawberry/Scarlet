// PL011 UART driver for ARM platforms (QEMU virt, etc.)

use alloc::{boxed::Box, collections::VecDeque, sync::Arc};
use core::any::Any;
use spin::{Mutex, RwLock};

use crate::arch::early_putc;
use crate::initcall::early;
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

// PL011 UART register offsets
const UARTDR: usize = 0x000; // Data Register
const UARTRSR: usize = 0x004; // Receive Status Register
const UARTFR: usize = 0x018; // Flag Register
const UARTIBRD: usize = 0x024; // Integer Baud Rate Divisor
const UARTFBRD: usize = 0x028; // Fractional Baud Rate Divisor
const UARTLCR_H: usize = 0x02C; // Line Control Register
const UARTCR: usize = 0x030; // Control Register
const UARTIMSC: usize = 0x038; // Interrupt Mask Set/Clear Register
const UARTRIS: usize = 0x03C; // Raw Interrupt Status Register
const UARTICR: usize = 0x044; // Interrupt Clear Register

// Flag Register bits
const FR_TXFE: u32 = 1 << 7; // Transmit FIFO empty
const FR_RXFF: u32 = 1 << 6; // Receive FIFO full
const FR_TXFF: u32 = 1 << 5; // Transmit FIFO full
const FR_RXFE: u32 = 1 << 4; // Receive FIFO empty
const FR_BUSY: u32 = 1 << 3; // UART busy

// Control Register bits
const CR_RXE: u32 = 1 << 9; // Receive enable
const CR_TXE: u32 = 1 << 8; // Transmit enable
const CR_UARTEN: u32 = 1 << 0; // UART enable

// Line Control Register bits
const LCR_H_WLEN_8: u32 = 3 << 5; // 8-bit word length
const LCR_H_FEN: u32 = 1 << 4; // Enable FIFOs

// Interrupt bits
const IMSC_RXIM: u32 = 1 << 4; // Receive interrupt mask

pub struct Pl011Uart {
    base: usize,
    interrupt_id: RwLock<Option<InterruptId>>,
    rx_buffer: Mutex<VecDeque<u8>>,
    event_emitter: Mutex<DeviceEventEmitter>,
    tx_lock: Mutex<()>,
}

impl Pl011Uart {
    pub fn new(base: usize) -> Self {
        Pl011Uart {
            base,
            interrupt_id: RwLock::new(None),
            rx_buffer: Mutex::new(VecDeque::new()),
            event_emitter: Mutex::new(DeviceEventEmitter::new()),
            tx_lock: Mutex::new(()),
        }
    }

    pub fn init(&self) {
        // Disable UART first
        self.reg_write(UARTCR, 0);

        // Wait for any current transmission to complete
        while self.reg_read(UARTFR) & FR_BUSY != 0 {}

        // Flush the transmit FIFO
        self.reg_write(UARTLCR_H, self.reg_read(UARTLCR_H) & !LCR_H_FEN);

        // Clear all pending interrupts
        self.reg_write(UARTICR, 0x7FF);

        // Set baud rate divisors (for 115200 baud with 24MHz clock)
        // IBRD = integer part of (24000000 / (16 * 115200)) = 13
        // FBRD = fractional part = round((0.0208 * 64) + 0.5) = 1
        self.reg_write(UARTIBRD, 13);
        self.reg_write(UARTFBRD, 1);

        // Set line control: 8 bits, no parity, 1 stop bit, FIFOs enabled
        self.reg_write(UARTLCR_H, LCR_H_WLEN_8 | LCR_H_FEN);

        // Enable UART, transmit, and receive
        self.reg_write(UARTCR, CR_UARTEN | CR_TXE | CR_RXE);
    }

    /// Enable UART-side interrupts after the controller line has been registered.
    pub fn enable_interrupts(&self, interrupt_id: InterruptId) -> Result<(), &'static str> {
        self.interrupt_id.write().replace(interrupt_id);

        // Enable receive interrupt
        self.reg_write(UARTIMSC, IMSC_RXIM);

        Ok(())
    }

    fn reg_write(&self, offset: usize, value: u32) {
        let addr = self.base + offset;
        unsafe { crate::arch::mmio::write32(addr, value) }
    }

    fn reg_read(&self, offset: usize) -> u32 {
        let addr = self.base + offset;
        unsafe { crate::arch::mmio::read32(addr) }
        // unsafe { core::ptr::read_volatile(addr as *const u32) }
    }

    fn write_byte_internal(&self, c: u8) {
        // // Wait until transmit FIFO is not full
        while self.reg_read(UARTFR) & FR_TXFF != 0 {
            core::hint::spin_loop();
        }
        self.reg_write(UARTDR, c as u32);
        // early_putc(c);
    }

    fn read_byte_internal(&self) -> Option<u8> {
        // Check if receive FIFO is empty
        if self.reg_read(UARTFR) & FR_RXFE != 0 {
            return None;
        }
        Some((self.reg_read(UARTDR) & 0xFF) as u8)
    }

    fn can_read(&self) -> bool {
        // RX FIFO is not empty
        self.reg_read(UARTFR) & FR_RXFE == 0
    }

    fn can_write(&self) -> bool {
        // TX FIFO is not full
        self.reg_read(UARTFR) & FR_TXFF == 0
    }
}

impl MemoryMappingOps for Pl011Uart {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for UART")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

static PL011_CAPS: [crate::device::DeviceCapability; 1] = [crate::device::DeviceCapability::Serial];

impl Device for Pl011Uart {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "pl011-uart"
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
        &PL011_CAPS
    }

    fn as_event_capable(&self) -> Option<&dyn EventCapableDevice> {
        Some(self)
    }
}

impl CharDevice for Pl011Uart {
    fn read_byte(&self) -> Option<u8> {
        self.rx_buffer.lock().pop_front()
    }

    /// Write a single byte. The byte itself is atomic under `tx_lock`,
    /// but consecutive `write_byte` calls are NOT guaranteed to be atomic.
    /// Use `write()` for multi-byte atomicity.
    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        let _lock = self.tx_lock.lock();
        self.write_byte_internal(byte);
        Ok(())
    }

    /// Write entire buffer atomically under `tx_lock`.
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        let _lock = self.tx_lock.lock();
        for &byte in buffer {
            self.write_byte_internal(byte);
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

impl ControlOps for Pl011Uart {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl EventCapableDevice for Pl011Uart {
    fn register_event_listener(&self, listener: alloc::sync::Weak<dyn DeviceEventListener>) {
        self.event_emitter.lock().register_listener(listener);
    }

    fn unregister_event_listener(&self, _listener_id: &str) {}

    fn emit_event(&self, event: &dyn crate::device::events::DeviceEvent) {
        self.event_emitter.lock().emit(event);
    }
}

impl InterruptCapableDevice for Pl011Uart {
    fn handle_interrupt(&self) -> crate::interrupt::InterruptResult<()> {
        // Read and clear interrupt status
        let ris = self.reg_read(UARTRIS);

        // Clear ALL interrupts first to prevent re-triggering
        self.reg_write(UARTICR, 0x7FF);

        if ris & IMSC_RXIM != 0 {
            // Receive interrupt - read all available data
            while let Some(c) = self.read_byte_internal() {
                // Emit received character event
                self.emit_event(&InputEvent { data: c });
                // Also store in buffer
                self.rx_buffer.lock().push_back(c);
            }
        }

        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        self.interrupt_id.read().clone()
    }
}

impl Selectable for Pl011Uart {
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

fn register_pl011() {
    use alloc::vec;

    let driver = Box::new(PlatformDeviceDriver::new(
        "pl011-uart-driver",
        pl011_probe,
        pl011_remove,
        vec!["arm,pl011"],
    ));

    // Register with Core priority since UART is essential
    DeviceManager::get_manager().register_driver(driver, DriverPriority::Core);
}

fn pl011_probe(device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    crate::early_println!("Probing PL011 UART device: {}", device_info.name());

    let memory_resource = device_info
        .get_resources()
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("No memory resource found for PL011")?;

    let paddr = memory_resource.start;
    let size = memory_resource.end - memory_resource.start + 1;
    crate::early_println!("PL011 paddr: {:#x}, size: {:#x}", paddr, size);

    // Map the PL011's physical MMIO region into the kernel virtual address space.
    let base_addr = crate::vm::ioremap(paddr, size).map_err(|e| {
        crate::early_println!("PL011 ioremap({:#x}, {:#x}) failed: {}", paddr, size, e);
        e
    })?;
    crate::early_println!("PL011 base address (virt): {:#x}", base_addr);

    let uart = Arc::new(Pl011Uart::new(base_addr));

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
        .map_err(|_| "Failed to register PL011 interrupt")?;

        crate::early_println!("PL011 interrupt ID: {}", uart_interrupt_id);

        if let Err(e) = uart.enable_interrupts(uart_interrupt_id) {
            crate::early_println!("Failed to enable PL011 interrupts: {}", e);
        } else {
            crate::early_println!("PL011 interrupts enabled (ID: {})", uart_interrupt_id);
            crate::early_println!("PL011 interrupt device registered");
        }
    } else {
        crate::early_println!("No interrupt resource found for PL011, using polling mode");
    }

    let device_id = DeviceManager::get_manager().register_device(uart);
    crate::early_println!("PL011 UART device registered with ID: {}", device_id);

    Ok(())
}

fn pl011_remove(_device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

// Only compile for AArch64
#[cfg(target_arch = "aarch64")]
driver_initcall!(register_pl011);
