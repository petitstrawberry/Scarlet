//! Qualcomm GENI (QUP) serial-engine UART.
//!
//! Register behavior was cross-checked against OpenBSD's `qcuart` driver and
//! Qualcomm's `GeniSerialPortLib` in TianoCore. The integration follows
//! Scarlet's existing character-device and interrupt-driver abstractions.

use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec};
use core::any::Any;

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
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
    interrupt::{InterruptClaim, InterruptId},
    object::capability::{ControlOps, MemoryMappingOps, Selectable},
};

const GENI_STATUS: usize = 0x040;
const GENI_STATUS_M_CMD_ACTIVE: u32 = 1 << 0;
const GENI_UART_TX_TRANS_LEN: usize = 0x270;

const GENI_M_CMD0: usize = 0x600;
const GENI_M_CMD0_UART_START_TX: u32 = 1 << 27;
const GENI_M_IRQ_STATUS: usize = 0x610;
const GENI_M_IRQ_EN: usize = 0x614;
const GENI_M_IRQ_CLEAR: usize = 0x618;
const GENI_M_IRQ_CMD_DONE: u32 = 1 << 0;
const GENI_M_IRQ_TX_FIFO_WATERMARK: u32 = 1 << 30;
const GENI_M_IRQ_SEC_IRQ: u32 = 1 << 31;

const GENI_S_CMD0: usize = 0x630;
const GENI_S_CMD0_UART_START_RX: u32 = 1 << 27;
const GENI_S_IRQ_STATUS: usize = 0x640;
const GENI_S_IRQ_EN: usize = 0x644;
const GENI_S_IRQ_CLEAR: usize = 0x648;
const GENI_S_IRQ_RX_FIFO_WATERMARK: u32 = 1 << 26;
const GENI_S_IRQ_RX_FIFO_LAST: u32 = 1 << 27;

const GENI_TX_FIFO: usize = 0x700;
const GENI_RX_FIFO: usize = 0x780;
const GENI_RX_FIFO_STATUS: usize = 0x804;
const GENI_RX_FIFO_STATUS_WC_MASK: u32 = 0x01ff_ffff;
const GENI_TX_FIFO_WATERMARK: usize = 0x80c;
const GENI_TX_WATERMARK: u32 = 2;

fn reg_read_at(base: usize, offset: usize) -> u32 {
    // SAFETY: callers provide a Device-typed mapping of the GENI register
    // window and all offsets used here are defined registers within it.
    unsafe { crate::arch::mmio::read32(base + offset) }
}

fn reg_write_at(base: usize, offset: usize, value: u32) {
    // SAFETY: callers provide a Device-typed mapping of the GENI register
    // window and all offsets used here are defined registers within it.
    unsafe { crate::arch::mmio::write32(base + offset, value) }
}

fn wait_for_at(base: usize, offset: usize, mask: u32) {
    while reg_read_at(base, offset) & mask == 0 {
        core::hint::spin_loop();
    }
}

/// Emit one byte with the firmware-configured GENI polling protocol.
///
/// This lock-free primitive is shared by the early/emergency console and the
/// runtime character device. Callers that require non-interleaved output must
/// provide their own serialization.
///
/// # Arguments
///
/// * `base` - Device-typed virtual base of the GENI serial engine.
/// * `byte` - Byte to transmit.
pub(crate) fn early_write_byte(base: usize, byte: u8) {
    reg_write_at(base, GENI_TX_FIFO_WATERMARK, GENI_TX_WATERMARK);
    reg_write_at(base, GENI_UART_TX_TRANS_LEN, 1);
    reg_write_at(base, GENI_M_CMD0, GENI_M_CMD0_UART_START_TX);

    wait_for_at(base, GENI_M_IRQ_STATUS, GENI_M_IRQ_TX_FIFO_WATERMARK);
    reg_write_at(base, GENI_TX_FIFO, byte as u32);
    reg_write_at(base, GENI_M_IRQ_CLEAR, GENI_M_IRQ_TX_FIFO_WATERMARK);

    wait_for_at(base, GENI_M_IRQ_STATUS, GENI_M_IRQ_CMD_DONE);
    reg_write_at(base, GENI_M_IRQ_CLEAR, GENI_M_IRQ_CMD_DONE);
}

struct QcomGeniUart {
    base: usize,
    interrupt_id: IrqRwSpinLock<Option<InterruptId>>,
    rx_buffer: IrqSpinLock<VecDeque<u8>>,
    event_emitter: IrqSpinLock<DeviceEventEmitter>,
    tx_lock: IrqSpinLock<()>,
}

impl QcomGeniUart {
    fn new(base: usize) -> Self {
        Self {
            base,
            interrupt_id: IrqRwSpinLock::new(None),
            rx_buffer: IrqSpinLock::new(VecDeque::new()),
            event_emitter: IrqSpinLock::new(DeviceEventEmitter::new()),
            tx_lock: IrqSpinLock::new(()),
        }
    }

    fn init(&self) {
        self.reg_write(GENI_M_IRQ_EN, 0);
        self.reg_write(GENI_S_IRQ_EN, 0);
        self.reg_write(GENI_M_IRQ_CLEAR, self.reg_read(GENI_M_IRQ_STATUS));
        self.reg_write(GENI_S_IRQ_CLEAR, self.reg_read(GENI_S_IRQ_STATUS));
    }

    /// Enable UART-side interrupts after the controller line has been registered.
    fn enable_interrupts(&self, interrupt_id: InterruptId) -> Result<(), &'static str> {
        self.interrupt_id.write().replace(interrupt_id);

        self.reg_write(
            GENI_S_IRQ_EN,
            GENI_S_IRQ_RX_FIFO_WATERMARK | GENI_S_IRQ_RX_FIFO_LAST,
        );
        self.reg_write(GENI_M_IRQ_EN, GENI_M_IRQ_SEC_IRQ);
        self.reg_write(GENI_S_CMD0, GENI_S_CMD0_UART_START_RX);

        Ok(())
    }

    fn reg_write(&self, offset: usize, value: u32) {
        reg_write_at(self.base, offset, value);
    }

    fn reg_read(&self, offset: usize) -> u32 {
        reg_read_at(self.base, offset)
    }

    fn write_byte_internal(&self, byte: u8) {
        early_write_byte(self.base, byte);
    }

    fn read_byte_internal(&self) -> Option<u8> {
        if self.reg_read(GENI_RX_FIFO_STATUS) & GENI_RX_FIFO_STATUS_WC_MASK == 0 {
            return None;
        }

        Some(self.reg_read(GENI_RX_FIFO) as u8)
    }

    fn drain_rx(&self) {
        while let Some(byte) = self.read_byte_internal() {
            self.emit_event(&InputEvent { data: byte });
            self.rx_buffer.lock().push_back(byte);
        }
    }
}

impl MemoryMappingOps for QcomGeniUart {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported for UART")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

static GENI_CAPS: [crate::device::DeviceCapability; 1] = [crate::device::DeviceCapability::Serial];

impl Device for QcomGeniUart {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "qcom-geni-uart"
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
        &GENI_CAPS
    }

    fn as_event_capable(&self) -> Option<&dyn EventCapableDevice> {
        Some(self)
    }
}

impl CharDevice for QcomGeniUart {
    fn read_byte(&self) -> Option<u8> {
        self.rx_buffer.lock().pop_front()
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        let _lock = self.tx_lock.lock();
        self.write_byte_internal(byte);
        Ok(())
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        if buffer.is_empty() {
            return Ok(0);
        }

        let _lock = self.tx_lock.lock();
        for &byte in buffer {
            self.write_byte_internal(byte);
        }

        Ok(buffer.len())
    }

    fn can_read(&self) -> bool {
        !self.rx_buffer.lock().is_empty()
    }

    fn can_write(&self) -> bool {
        self.reg_read(GENI_STATUS) & GENI_STATUS_M_CMD_ACTIVE == 0
    }
}

impl ControlOps for QcomGeniUart {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl EventCapableDevice for QcomGeniUart {
    fn register_event_listener(&self, listener: alloc::sync::Weak<dyn DeviceEventListener>) {
        self.event_emitter.lock().register_listener(listener);
    }

    fn unregister_event_listener(&self, _listener_id: &str) {}

    fn emit_event(&self, event: &dyn crate::device::events::DeviceEvent) {
        self.event_emitter.lock().emit(event);
    }
}

impl InterruptCapableDevice for QcomGeniUart {
    fn handle_interrupt(&self) -> crate::interrupt::InterruptResult<()> {
        let _ = self.claim_interrupt()?;
        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        *self.interrupt_id.read()
    }

    fn claim_interrupt(&self) -> crate::interrupt::InterruptResult<InterruptClaim> {
        let m_status = self.reg_read(GENI_M_IRQ_STATUS);
        if m_status & GENI_M_IRQ_SEC_IRQ == 0 {
            return Ok(InterruptClaim::NotMine);
        }

        let s_status = self.reg_read(GENI_S_IRQ_STATUS);
        self.reg_write(GENI_S_IRQ_CLEAR, s_status);
        self.reg_write(GENI_M_IRQ_CLEAR, GENI_M_IRQ_SEC_IRQ);
        self.drain_rx();

        Ok(InterruptClaim::Handled)
    }
}

impl Selectable for QcomGeniUart {
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

fn register_qcom_geni_uart() {
    let driver = Box::new(PlatformDeviceDriver::new(
        "qcom-geni-uart-driver",
        qcom_geni_probe,
        qcom_geni_remove,
        vec!["qcom,geni-debug-uart"],
    ));

    DeviceManager::get_manager().register_driver(driver, DriverPriority::Core);
}

fn qcom_geni_probe(device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    crate::early_println!("Probing Qualcomm GENI UART device: {}", device_info.name());

    let memory_resource = device_info
        .get_resources()
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("No memory resource found for GENI UART")?;
    let paddr = memory_resource.start;
    let size = memory_resource.end - memory_resource.start + 1;

    let base = crate::vm::ioremap(paddr, size).map_err(|e| {
        crate::early_println!("GENI UART ioremap({:#x}, {:#x}) failed: {}", paddr, size, e);
        e
    })?;

    let uart = Arc::new(QcomGeniUart::new(base));
    uart.init();

    if let Some(irq_resource) = device_info
        .get_resources()
        .iter()
        .find(|resource| resource.res_type == PlatformDeviceResourceType::IRQ)
    {
        let interrupt_id = crate::interrupt::register_and_enable_platform_irq_device(
            irq_resource,
            uart.clone(),
            crate::arch::get_cpu().get_cpuid() as u32,
        )
        .map_err(|_| "Failed to register GENI UART interrupt")?;

        uart.enable_interrupts(interrupt_id)?;
        crate::early_println!("GENI UART interrupts enabled (ID: {})", interrupt_id);
    } else {
        crate::early_println!("No interrupt resource found for GENI UART, output only");
    }

    #[cfg(target_arch = "aarch64")]
    crate::arch::aarch64::earlycon::register_runtime_qcom_geni(base);

    let device_id = DeviceManager::get_manager().register_device(uart);
    crate::early_println!("GENI UART device registered with ID: {}", device_id);

    Ok(())
}

fn qcom_geni_remove(_device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

#[cfg(target_arch = "aarch64")]
driver_initcall!(register_qcom_geni_uart);
