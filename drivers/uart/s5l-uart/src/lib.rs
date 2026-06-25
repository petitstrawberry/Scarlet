#![no_std]

extern crate alloc;

use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec};
use core::fmt;
use scarlet::sync::{Mutex, RwLock};

use scarlet::{
    arch::mmio,
    device::{
        Device, DeviceCapability, DeviceInfo, DeviceType,
        char::CharDevice,
        clk::ClkHandle,
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
    interrupt::{InterruptId, InterruptResult},
    object::capability::{ControlOps, MemoryMappingOps, Selectable},
};

// =============================================================================
// S5L UART Register Offsets
// =============================================================================

const ULCON: usize = 0x000; // Line Control Register
const UCON: usize = 0x004; // Control Register
const UFCON: usize = 0x008; // FIFO Control Register
const UMCON: usize = 0x00C; // Modem Control Register
const UTRSTAT: usize = 0x010; // TX/RX Status Register
const UERSTAT: usize = 0x014; // Error Status Register
const UFSTAT: usize = 0x018; // FIFO Status Register
const UMSTAT: usize = 0x01C; // Modem Status Register
const UTXH: usize = 0x020; // TX Holding Register (write)
const URXH: usize = 0x024; // RX Holding Register (read)
const UBRDIV: usize = 0x028; // Baud Rate Divisor Register
const UFRACVAL: usize = 0x02C; // Fractional Baud Rate Register

// =============================================================================
// UCON (Control Register) bits
// =============================================================================

const UCON_TXTHRESH_ENA: u32 = 1 << 13;
const UCON_RXTHRESH_ENA: u32 = 1 << 12;
const UCON_RXTO_ENA: u32 = 1 << 9;
const UCON_TXMODE_MASK: u32 = 0x0C;
const UCON_RXMODE_MASK: u32 = 0x03;
const UCON_MODE_IRQ: u32 = 1;

// =============================================================================
// UTRSTAT (TX/RX Status Register) bits
// =============================================================================

const UTRSTAT_RXTO: u32 = 1 << 9;
const UTRSTAT_TXTHRESH: u32 = 1 << 5;
const UTRSTAT_RXTHRESH: u32 = 1 << 4;
const UTRSTAT_TXE: u32 = 1 << 2;
const UTRSTAT_TXBE: u32 = 1 << 1;
const UTRSTAT_RXD: u32 = 1 << 0;

// =============================================================================
// UFSTAT (FIFO Status Register) bits
// =============================================================================

const UFSTAT_TXFULL: u32 = 1 << 9;
const UFSTAT_RXFULL: u32 = 1 << 8;
const UFSTAT_TXCNT_SHIFT: u32 = 4;
const UFSTAT_TXCNT_MASK: u32 = 0xF0;
const UFSTAT_RXCNT_MASK: u32 = 0x0F;

// =============================================================================
// S5L UART Device
// =============================================================================

static S5L_CAPS: [DeviceCapability; 1] = [DeviceCapability::Serial];

pub struct S5lUart {
    base: usize,
    _uart_clk: Option<ClkHandle>,
    _baud_clk: Option<ClkHandle>,
    interrupt_id: RwLock<Option<InterruptId>>,
    tx_lock: Mutex<()>,
    rx_buffer: Mutex<VecDeque<u8>>,
    event_emitter: Mutex<DeviceEventEmitter>,
}

impl S5lUart {
    pub fn new(base: usize, uart_clk: Option<ClkHandle>, baud_clk: Option<ClkHandle>) -> Self {
        S5lUart {
            base,
            _uart_clk: uart_clk,
            _baud_clk: baud_clk,
            interrupt_id: RwLock::new(None),
            tx_lock: Mutex::new(()),
            rx_buffer: Mutex::new(VecDeque::new()),
            event_emitter: Mutex::new(DeviceEventEmitter::new()),
        }
    }

    pub fn init(&self) {
        let ucon = (UCON_MODE_IRQ << 2) | UCON_MODE_IRQ;
        self.reg_write(UCON, ucon);

        self.reg_write(UFCON, 0x07);

        self.reg_write(ULCON, 0x03);

        self.reg_write(UBRDIV, 13);
        self.reg_write(UFRACVAL, 1);
    }

    pub fn enable_interrupts(&self, interrupt_id: InterruptId) -> Result<(), &'static str> {
        self.interrupt_id.write().replace(interrupt_id);

        let ucon = self.reg_read(UCON);
        self.reg_write(UCON, ucon | UCON_RXTHRESH_ENA | UCON_RXTO_ENA);

        scarlet::interrupt::InterruptManager::global()
            .enable_external_interrupt(interrupt_id, 0)
            .map_err(|_| "Failed to enable interrupt")?;

        Ok(())
    }

    fn reg_write(&self, offset: usize, value: u32) {
        let addr = self.base + offset;
        unsafe { mmio::write32(addr, value) }
    }

    fn reg_read(&self, offset: usize) -> u32 {
        let addr = self.base + offset;
        unsafe { mmio::read32(addr) }
    }

    fn write_byte_internal(&self, byte: u8) {
        while self.reg_read(UFSTAT) & UFSTAT_TXFULL != 0 {
            core::hint::spin_loop();
        }
        self.reg_write(UTXH, byte as u32);
    }

    fn read_byte_internal(&self) -> Option<u8> {
        let ufstat = self.reg_read(UFSTAT);
        if ufstat & UFSTAT_RXCNT_MASK != 0 {
            let data = self.reg_read(URXH);
            Some(data as u8)
        } else {
            None
        }
    }

    fn can_read(&self) -> bool {
        !self.rx_buffer.lock().is_empty()
    }

    fn can_write_hw(&self) -> bool {
        self.reg_read(UFSTAT) & UFSTAT_TXFULL == 0
    }
}

impl fmt::Write for S5lUart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            if c == '\n' {
                self.write_byte_internal(b'\r');
            }
            self.write_byte_internal(c as u8);
        }
        Ok(())
    }
}

impl MemoryMappingOps for S5lUart {
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

impl Device for S5lUart {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "s5l-uart"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }

    fn capabilities(&self) -> &'static [DeviceCapability] {
        &S5L_CAPS
    }

    fn as_event_capable(&self) -> Option<&dyn EventCapableDevice> {
        Some(self)
    }
}

impl CharDevice for S5lUart {
    fn read_byte(&self) -> Option<u8> {
        self.rx_buffer.lock().pop_front()
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        let _lock = self.tx_lock.lock();
        self.write_byte_internal(byte);
        Ok(())
    }

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
        self.can_write_hw()
    }
}

impl ControlOps for S5lUart {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl EventCapableDevice for S5lUart {
    fn register_event_listener(&self, listener: alloc::sync::Weak<dyn DeviceEventListener>) {
        self.event_emitter.lock().register_listener(listener);
    }

    fn unregister_event_listener(&self, _listener_id: &str) {}

    fn emit_event(&self, event: &dyn scarlet::device::events::DeviceEvent) {
        self.event_emitter.lock().emit(event);
    }
}

impl InterruptCapableDevice for S5lUart {
    fn handle_interrupt(&self) -> InterruptResult<()> {
        let utrstat = self.reg_read(UTRSTAT);

        if utrstat & (UTRSTAT_RXTHRESH | UTRSTAT_RXTO) != 0 {
            let mut count = 0;
            while let Some(c) = self.read_byte_internal() {
                self.emit_event(&InputEvent { data: c });
                self.rx_buffer.lock().push_back(c);
                count += 1;

                if count > 128 {
                    scarlet::early_println!(
                        "[S5L] Warning: read limit reached in interrupt handler"
                    );
                    break;
                }
            }

            self.reg_write(UTRSTAT, UTRSTAT_RXTHRESH | UTRSTAT_RXTO);
        }

        if utrstat & UTRSTAT_TXTHRESH != 0 {
            self.reg_write(UTRSTAT, UTRSTAT_TXTHRESH);
        }

        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        self.interrupt_id.read().clone()
    }
}

impl Selectable for S5lUart {
    fn wait_until_ready(
        &self,
        _interest: scarlet::object::capability::selectable::ReadyInterest,
        _trapframe: &mut scarlet::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> scarlet::object::capability::selectable::SelectWaitOutcome {
        scarlet::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

unsafe impl Send for S5lUart {}
unsafe impl Sync for S5lUart {}

// =============================================================================
// Platform Device Driver Registration
// =============================================================================

/// Probe an S5L UART, enabling optional UART and baud clocks before MMIO setup.
fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    scarlet::early_println!("[S5L] probe: probing device {}", device.name());

    let mem_resources: alloc::vec::Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let paddr = mem_resources
        .get(0)
        .map(|r| r.start)
        .ok_or("No memory resource found for S5L UART")?;
    let size = mem_resources
        .get(0)
        .map(|r| r.end - r.start + 1)
        .unwrap_or(0x1000);

    scarlet::early_println!("[S5L] probe: paddr={:#x}, size={:#x}", paddr, size);

    let base_addr = scarlet::vm::ioremap(paddr, size).map_err(|e| {
        scarlet::early_println!("[S5L] probe: ioremap failed: {}", e);
        e
    })?;

    scarlet::early_println!("[S5L] probe: mapped to {:#x}", base_addr);

    let uart_clk = match DeviceManager::get_manager().resolve_clk(device, "uart") {
        Ok(handle) => {
            let _ = handle.prepare_enable();
            Some(handle)
        }
        Err(e) => {
            scarlet::early_println!("[S5L] warning: uart clock unavailable: {}", e);
            None
        }
    };
    let baud_clk = match DeviceManager::get_manager().resolve_clk(device, "clk_uart_baud0") {
        Ok(handle) => {
            let _ = handle.prepare_enable();
            Some(handle)
        }
        Err(e) => {
            scarlet::early_println!("[S5L] warning: baud clock unavailable: {}", e);
            None
        }
    };

    let uart = Arc::new(S5lUart::new(base_addr, uart_clk, baud_clk));
    uart.init();

    let irq_resources: alloc::vec::Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::IRQ))
        .collect();

    if let Some(irq_res) = irq_resources.get(0) {
        let interrupt_id = irq_res.start as InterruptId;
        scarlet::early_println!("[S5L] probe: interrupt ID={}", interrupt_id);

        if let Err(e) = uart.enable_interrupts(interrupt_id) {
            scarlet::early_println!("[S5L] probe: failed to enable interrupts: {}", e);
        } else {
            scarlet::early_println!("[S5L] probe: interrupts enabled");

            if let Err(e) = scarlet::interrupt::InterruptManager::global()
                .register_interrupt_device(interrupt_id, uart.clone())
            {
                scarlet::early_println!("[S5L] probe: failed to register interrupt device: {}", e);
            } else {
                scarlet::early_println!("[S5L] probe: interrupt device registered");
            }
        }
    } else {
        scarlet::early_println!("[S5L] probe: no interrupt resource, polling mode");
    }

    let device_id = DeviceManager::get_manager().register_device(uart);
    scarlet::early_println!("[S5L] probe: device registered with ID={}", device_id);

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_s5l_uart() {
    scarlet::early_println!("[S5L] register_driver: registering S5L UART driver");

    let driver = Box::new(PlatformDeviceDriver::new(
        "s5l-uart-driver",
        probe_fn,
        remove_fn,
        vec!["apple,s5l-uart"],
    ));

    DeviceManager::get_manager().register_driver(driver, DriverPriority::Core);
}

scarlet::driver_initcall!(register_s5l_uart);

#[used]
static SCARLET_DRIVER_S5L_UART_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
