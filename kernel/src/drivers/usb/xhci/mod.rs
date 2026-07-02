//! xHCI (USB 3.0) Host Controller Driver
//!
//! This module implements the xHCI host controller driver for Scarlet kernel.
//! It supports PCI-based xHCI controllers and provides USB device enumeration
//! and HID boot protocol support.

pub mod context;
pub mod registers;
pub mod ring;
pub mod trb;

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::mem::size_of;
use core::ptr::{read_unaligned, read_volatile, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering, fence};
use spin::Mutex;

use crate::device::Device;
use crate::device::block::{
    BlockDevice,
    request::{BlockIORequest, BlockIORequestType, BlockIOResult},
};
use crate::device::events::InterruptCapableDevice;
use crate::device::iommu::{DmaContext, IommuMapFlags};
use crate::device::manager::{DeviceManager, DriverPriority};
use crate::device::pci::config::{self, PciConfig};
use crate::device::pci::device::PciDeviceInfo;
use crate::device::pci::driver::{PciDeviceDriver, PciDeviceId};
use crate::device::pci::intx::PciIntxInterruptSource;
use crate::device::usb::UsbHostController;
use crate::driver_initcall;
use crate::drivers::usb::core::descriptor::{
    ConfigurationDescriptor, DescriptorHeader, DeviceDescriptor, EndpointDescriptor,
    InterfaceDescriptor,
};
use crate::drivers::usb::core::device::{UsbDevice, UsbDeviceState, UsbSpeed};
use crate::drivers::usb::hid::boot::{
    HidBootProtocol, HidKeyboardDevice, HidMouseDevice, KeyboardBootReport, MouseBootReport,
    boot_protocol_for_interface,
};
use crate::drivers::usb::xhci::context::{
    DeviceContextBuffer, InputContext, InputContextBuffer, address_input_context_bytes,
    device_context_bytes, full_input_context_bytes, speed as ctx_speed,
};
use crate::drivers::usb::xhci::registers::{RegisterSpace, capability, operational};
use crate::drivers::usb::xhci::ring::{DmaTrbRing, EventRing};
use crate::drivers::usb::xhci::trb::{Trb, TrbType};
use crate::interrupt::{InterruptClaim, InterruptId, InterruptManager};
use crate::mem::page::ContiguousPages;
use crate::object::capability::{ControlOps, MemoryMappingInfo, MemoryMappingOps, Selectable};
use crate::println;
use crate::timer::{TimerHandler, add_timer, get_tick, ms_to_ticks};
use crate::vm;

const COMMAND_RING_TRBS: usize = 256;
const EVENT_RING_TRBS: usize = 256;
const COMMAND_COMPLETION_SUCCESS: u8 = 1;
const TRANSFER_EVENT_SHORT_PACKET: u8 = 13;
const USBSTS_EVENT_INTERRUPT: u32 = 1 << 3;
const USBSTS_HOST_SYSTEM_ERROR: u32 = 1 << 2;
const USBSTS_PORT_CHANGE_DETECT: u32 = 1 << 4;
const USBSTS_WRITE_1_TO_CLEAR: u32 =
    USBSTS_HOST_SYSTEM_ERROR | USBSTS_EVENT_INTERRUPT | USBSTS_PORT_CHANGE_DETECT;
const ERDP_EVENT_HANDLER_BUSY: u64 = 1 << 3;
const USB_REQ_GET_DESCRIPTOR: u8 = 0x06;
const USB_REQ_GET_STATUS: u8 = 0x00;
const USB_REQ_CLEAR_FEATURE: u8 = 0x01;
const USB_REQ_SET_FEATURE: u8 = 0x03;
const USB_REQ_SET_CONFIGURATION: u8 = 0x09;
const USB_REQ_SET_INTERFACE: u8 = 0x0b;
const USB_REQ_SET_PROTOCOL: u8 = 0x0b;
const USB_DT_DEVICE: u8 = 1;
const USB_DT_CONFIGURATION: u8 = 2;
const USB_DT_INTERFACE: u8 = 4;
const USB_DT_ENDPOINT: u8 = 5;
const USB_DT_HUB: u8 = 0x29;
const USB_CLASS_HID: u8 = 3;
const USB_CLASS_HUB: u8 = 0x09;
const USB_CLASS_MASS_STORAGE: u8 = 0x08;
const USB_MSC_SUBCLASS_SCSI: u8 = 0x06;
const USB_MSC_PROTOCOL_BULK_ONLY: u8 = 0x50;
const USB_ENDPOINT_XFER_INT: u8 = 3;
const USB_ENDPOINT_XFER_BULK: u8 = 2;
const EP0_DCI: u8 = 1;
const HCC_PARAMS1_AC64: u32 = 1 << 0;
const HCC_PARAMS1_CONTEXT_SIZE_64: u32 = 1 << 2;
const USB_BULK_MAX_TRANSFER: usize = 64 * 1024;
const XHCI_COMMAND_TIMEOUT_US: u64 = 500_000;
const XHCI_TRANSFER_TIMEOUT_US: u64 = 5_000_000;
const XHCI_PENDING_EVENT_LIMIT: usize = 64;
const XHCI_VERBOSE_TRACE: bool = false;
const USB_STORAGE_CBW_SIGNATURE: u32 = 0x4342_5355;
const USB_STORAGE_CSW_SIGNATURE: u32 = 0x5342_5355;
const USB_STORAGE_CBW_LEN: usize = 31;
const USB_STORAGE_CSW_LEN: usize = 13;
const SCSI_READ_10: u8 = 0x28;
const SCSI_WRITE_10: u8 = 0x2a;
const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PLS_SHIFT: u32 = 5;
const PORTSC_PLS_MASK: u32 = 0xf << PORTSC_PLS_SHIFT;
const PORTSC_PP: u32 = 1 << 9;
const PORTSC_SPEED_SHIFT: u32 = 10;
const PORTSC_SPEED_MASK: u32 = 0xf << PORTSC_SPEED_SHIFT;
const PORTSC_CSC: u32 = 1 << 17;
const PORTSC_PEC: u32 = 1 << 18;
const PORTSC_WRC: u32 = 1 << 19;
const PORTSC_OCC: u32 = 1 << 20;
const PORTSC_PRC: u32 = 1 << 21;
const PORTSC_PLC: u32 = 1 << 22;
const PORTSC_CEC: u32 = 1 << 23;
const PORTSC_CAS: u32 = 1 << 24;
const PORTSC_CHANGE_BITS: u32 =
    PORTSC_CSC | PORTSC_PEC | PORTSC_WRC | PORTSC_OCC | PORTSC_PRC | PORTSC_PLC | PORTSC_CEC;
const PORTSC_WRITE_PRESERVE_BITS: u32 = PORTSC_PP;
const PORT_RESET_TIMEOUT_US: u64 = 500_000;
const PORT_RESET_RECOVERY_US: u64 = 10_000;
const USB_HUB_POWER_RECOVERY_US: u64 = 20_000;
const USB_HUB_PORT_RESET_TIMEOUT_US: u64 = 500_000;
const USB_HUB_FEATURE_PORT_RESET: u16 = 4;
const USB_HUB_FEATURE_PORT_POWER: u16 = 8;
const USB_HUB_FEATURE_C_PORT_CONNECTION: u16 = 16;
const USB_HUB_FEATURE_C_PORT_ENABLE: u16 = 17;
const USB_HUB_FEATURE_C_PORT_RESET: u16 = 20;
const USB_HUB_PORT_CONNECTION: u16 = 1 << 0;
const USB_HUB_PORT_ENABLE: u16 = 1 << 1;
const USB_HUB_PORT_LOW_SPEED: u16 = 1 << 9;
const USB_HUB_PORT_HIGH_SPEED: u16 = 1 << 10;
const USB_HUB_PORT_CHANGE_CONNECTION: u16 = 1 << 0;
const USB_HUB_PORT_CHANGE_RESET: u16 = 1 << 4;
const USB_HUB_ROUTE_DEPTH_MAX: u8 = 5;

#[inline]
fn read_mmio64_lo_hi(addr: usize) -> u64 {
    // SAFETY: All callers pass a mapped xHCI 64-bit MMIO register address.
    // xHCI address registers support dword access and are read low dword first.
    unsafe {
        let low = read_volatile(addr as *const u32);
        let high = read_volatile((addr + 4) as *const u32);
        ((high as u64) << 32) | low as u64
    }
}

fn scsi_rw10_command(opcode: u8, lba: usize, blocks: usize) -> Result<[u8; 10], &'static str> {
    if lba > u32::MAX as usize {
        return Err("USB storage LBA exceeds SCSI(10) range");
    }
    if blocks == 0 || blocks > u16::MAX as usize {
        return Err("USB storage invalid SCSI(10) block count");
    }

    let lba_be = (lba as u32).to_be_bytes();
    let blocks_be = (blocks as u16).to_be_bytes();
    Ok([
        opcode,
        0,
        lba_be[0],
        lba_be[1],
        lba_be[2],
        lba_be[3],
        0,
        blocks_be[0],
        blocks_be[1],
        0,
    ])
}

#[inline]
fn write_mmio64_lo_hi(addr: usize, value: u64) {
    // SAFETY: All callers pass a mapped xHCI 64-bit MMIO register address.
    // xHCI address registers are written low dword first, then high dword.
    unsafe {
        write_volatile(addr as *mut u32, value as u32);
        write_volatile((addr + 4) as *mut u32, (value >> 32) as u32);
    }
}

fn sync_pages_for_device(pages: &ContiguousPages) {
    crate::arch::clean_dcache_to_poc_range(
        pages.as_vaddr(),
        pages.len() * crate::environment::PAGE_SIZE,
    );
}

fn sync_pages_before_device_write(pages: &ContiguousPages) {
    crate::arch::clean_invalidate_dcache_to_poc_range(
        pages.as_vaddr(),
        pages.len() * crate::environment::PAGE_SIZE,
    );
}

fn sync_pages_after_device_write(pages: &ContiguousPages) {
    crate::arch::invalidate_dcache_to_poc_range(
        pages.as_vaddr(),
        pages.len() * crate::environment::PAGE_SIZE,
    );
}

fn dma_rw_flags() -> IommuMapFlags {
    IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortStatus {
    connected: bool,
    enabled: bool,
    resetting: bool,
    link_state: u32,
    powered: bool,
    speed_raw: u32,
    change_bits: u32,
    config_error: bool,
}

impl PortStatus {
    const fn from_portsc(portsc: u32) -> Self {
        Self {
            connected: (portsc & PORTSC_CCS) != 0,
            enabled: (portsc & PORTSC_PED) != 0,
            resetting: (portsc & PORTSC_PR) != 0,
            link_state: (portsc & PORTSC_PLS_MASK) >> PORTSC_PLS_SHIFT,
            powered: (portsc & PORTSC_PP) != 0,
            speed_raw: (portsc & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT,
            change_bits: portsc & PORTSC_CHANGE_BITS,
            config_error: (portsc & PORTSC_CAS) != 0,
        }
    }

    const fn speed(self) -> UsbSpeed {
        match self.speed_raw {
            1 => UsbSpeed::Full,
            2 => UsbSpeed::Low,
            3 => UsbSpeed::High,
            4 => UsbSpeed::Super,
            _ => UsbSpeed::Full,
        }
    }
}

#[derive(Clone, Copy)]
struct BootInterfaceConfig {
    protocol: HidBootProtocol,
    configuration_value: u8,
    interface_number: u8,
    endpoint_address: u8,
    max_packet_size: u16,
    interval: u8,
}

#[derive(Clone, Copy)]
struct MassStorageInterfaceConfig {
    configuration_value: u8,
    interface_number: u8,
    bulk_in_endpoint: u8,
    bulk_in_max_packet_size: u16,
    bulk_out_endpoint: u8,
    bulk_out_max_packet_size: u16,
}

#[derive(Clone, Copy)]
struct HubInterfaceConfig {
    configuration_value: u8,
    interface_number: u8,
    alternate_setting: u8,
    protocol: u8,
}

#[derive(Clone, Copy)]
struct HubPortStatus {
    status: u16,
    change: u16,
}

struct BulkEndpointRuntime {
    endpoint_address: u8,
    dci: u8,
    max_packet_size: u16,
    ring: DmaTrbRing,
}

struct MassStorageRuntime {
    input_context: ContiguousPages,
    bulk_in: BulkEndpointRuntime,
    bulk_out: BulkEndpointRuntime,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct HubDescriptor {
    length: u8,
    descriptor_type: u8,
    num_ports: u8,
    characteristics: u16,
    power_on_to_power_good: u8,
    controller_current: u8,
}

impl HubDescriptor {
    const fn encoded_size() -> usize {
        size_of::<Self>()
    }
}

#[derive(Clone, Copy)]
struct HubRuntime {
    route_string: u32,
    depth: u8,
    num_ports: u8,
    multi_tt: bool,
    power_good_time_us: u64,
}

impl HubPortStatus {
    const fn connected(self) -> bool {
        (self.status & USB_HUB_PORT_CONNECTION) != 0
    }

    const fn enabled(self) -> bool {
        (self.status & USB_HUB_PORT_ENABLE) != 0
    }

    const fn reset_complete_changed(self) -> bool {
        (self.change & USB_HUB_PORT_CHANGE_RESET) != 0
    }

    const fn connection_changed(self) -> bool {
        (self.change & USB_HUB_PORT_CHANGE_CONNECTION) != 0
    }

    const fn speed(self) -> UsbSpeed {
        if (self.status & USB_HUB_PORT_LOW_SPEED) != 0 {
            UsbSpeed::Low
        } else if (self.status & USB_HUB_PORT_HIGH_SPEED) != 0 {
            UsbSpeed::High
        } else {
            UsbSpeed::Full
        }
    }
}

/// PCI base class for serial bus controllers.
pub const PCI_CLASS_SERIAL_BUS: u8 = 0x0c;
/// PCI subclass for USB controllers.
pub const PCI_SUBCLASS_USB: u8 = 0x03;
/// PCI programming interface identifying xHCI.
pub const PCI_PROG_IF_XHCI: u8 = 0x30;
/// Combined PCI class code for xHCI controllers.
pub const XHCI_CLASS_CODE: u32 = ((PCI_CLASS_SERIAL_BUS as u32) << 16)
    | ((PCI_SUBCLASS_USB as u32) << 8)
    | PCI_PROG_IF_XHCI as u32;

/// Decoded PCI MMIO BAR information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciBar {
    /// Physical base address of the BAR
    pub base: usize,
    /// Size of the BAR region (determined by writing 0xFFFFFFFF)
    pub size: usize,
    /// True if this is a memory BAR (not I/O)
    pub is_memory: bool,
    /// True if this is a 64-bit BAR
    pub is_64bit: bool,
    /// True if the region is prefetchable
    pub prefetchable: bool,
}

/// xHCI capability registers (read-only during operation)
#[derive(Debug, Clone, Copy)]
pub struct XhciCapabilities {
    /// Capability registers length (offset to operational registers)
    pub cap_length: u8,
    /// xHCI version (BCD encoded, e.g., 0x0100 = 1.0)
    pub hci_version: u16,
    /// Structural parameters 1 (slots, intrs, ports)
    pub hcs_params1: u32,
    /// Structural parameters 2 (IST, ERST max, scratchpad)
    pub hcs_params2: u32,
    /// Structural parameters 3 (max U1/U2 exit latency)
    pub hcs_params3: u32,
    /// Capability parameters 1 (64-bit, BW negotiation, etc.)
    pub hcc_params1: u32,
    /// Doorbell array offset
    pub dboff: u32,
    /// Runtime registers offset
    pub rtsoff: u32,
}

/// xHCI operational register access
pub struct XhciOperational {
    base: usize,
}

impl XhciOperational {
    /// Create operational register accessor
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    /// Read USB Command Register
    pub fn read_usbcmd(&self) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + operational::USBCMD) as *const u32) }
    }

    /// Write USB Command Register
    pub fn write_usbcmd(&self, value: u32) {
        unsafe {
            core::ptr::write_volatile((self.base + operational::USBCMD) as *mut u32, value);
        }
    }

    /// Read USB Status Register
    pub fn read_usbsts(&self) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + operational::USBSTS) as *const u32) }
    }

    /// Write USB Status Register (to clear bits)
    pub fn write_usbsts(&self, value: u32) {
        unsafe {
            core::ptr::write_volatile((self.base + operational::USBSTS) as *mut u32, value);
        }
    }

    /// Read Page Size Register
    pub fn read_pagesize(&self) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + operational::PAGESIZE) as *const u32) }
    }

    /// Read Device Notification Control Register
    pub fn read_dnctrl(&self) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + operational::DNCTRL) as *const u32) }
    }

    /// Write Device Notification Control Register
    pub fn write_dnctrl(&self, value: u32) {
        unsafe {
            core::ptr::write_volatile((self.base + operational::DNCTRL) as *mut u32, value);
        }
    }

    /// Read Command Ring Control Register
    pub fn read_crcr(&self) -> u64 {
        read_mmio64_lo_hi(self.base + operational::CRCR)
    }

    /// Write Command Ring Control Register
    pub fn write_crcr(&self, value: u64) {
        write_mmio64_lo_hi(self.base + operational::CRCR, value);
    }

    /// Read Device Context Base Address Array Pointer Register
    pub fn read_dcbaap(&self) -> u64 {
        read_mmio64_lo_hi(self.base + operational::DCBAAP)
    }

    /// Write Device Context Base Address Array Pointer Register
    pub fn write_dcbaap(&self, value: u64) {
        write_mmio64_lo_hi(self.base + operational::DCBAAP, value);
    }

    /// Read Configure Register
    pub fn read_config(&self) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + operational::CONFIG) as *const u32) }
    }

    /// Write Configure Register
    pub fn write_config(&self, value: u32) {
        unsafe {
            core::ptr::write_volatile((self.base + operational::CONFIG) as *mut u32, value);
        }
    }
}

/// xHCI controller state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XhciState {
    /// Controller not initialized
    Uninitialized,
    /// Controller halted
    Halted,
    /// Controller resetting
    Resetting,
    /// Controller running
    Running,
    /// Controller in error state
    Error,
}

enum HidDeviceState {
    Keyboard(HidKeyboardDevice),
    Mouse(HidMouseDevice),
}

struct SlotRuntime {
    usb_device: UsbDevice,
    route_string: u32,
    route_depth: u8,
    device_context: ContiguousPages,
    ep0_input_context: ContiguousPages,
    ep0_ring: DmaTrbRing,
    interrupt_input_context: Option<ContiguousPages>,
    interrupt_ring: Option<DmaTrbRing>,
    interrupt_buffer: Option<ContiguousPages>,
    interrupt_dci: Option<u8>,
    interrupt_endpoint_address: Option<u8>,
    interrupt_max_packet_size: Option<u16>,
    storage: Option<MassStorageRuntime>,
    hid: Option<HidDeviceState>,
    hub: Option<HubRuntime>,
}

struct ScratchpadBuffers {
    array: ContiguousPages,
    array_dma_addr: usize,
    buffers: Vec<ContiguousPages>,
}

impl ScratchpadBuffers {
    fn array_dma_addr(&self) -> usize {
        self.array_dma_addr
    }

    fn buffer_count(&self) -> usize {
        self.buffers.len()
    }

    fn sync_for_device(&self) {
        sync_pages_for_device(&self.array);
        for buffer in &self.buffers {
            sync_pages_for_device(buffer);
        }
    }
}

/// xHCI Controller instance
pub struct XhciController {
    mmio_base: usize,
    dma_context: DmaContext,
    regs: RegisterSpace,
    caps: XhciCapabilities,
    operational: XhciOperational,
    state: Mutex<XhciState>,
    max_slots: u8,
    max_ports: u8,
    max_intrs: u16,
    context_size: usize,
    dcbaa: Mutex<Option<ContiguousPages>>,
    scratchpads: Mutex<Option<ScratchpadBuffers>>,
    cmd_ring: Mutex<Option<DmaTrbRing>>,
    event_ring: Mutex<Option<EventRing>>,
    pending_events: Mutex<Vec<Trb>>,
    devices: Mutex<Vec<UsbDevice>>,
    slot_runtime: Mutex<Vec<SlotRuntime>>,
    interrupt_id: Mutex<Option<InterruptId>>,
}

impl XhciController {
    /// Create a new xHCI controller instance
    ///
    /// # Arguments
    ///
    /// * `mmio_base` - Virtual address of the MMIO region
    ///
    /// # Returns
    ///
    /// A new XhciController instance or an error string
    pub fn new(mmio_base: usize) -> Result<Self, &'static str> {
        Self::new_with_dma_context(mmio_base, DmaContext::direct())
    }

    /// Create a new xHCI controller instance with an explicit DMA context.
    ///
    /// # Arguments
    ///
    /// * `mmio_base` - Virtual address of the MMIO region.
    /// * `dma_context` - DMA mapping context for the xHCI requester.
    ///
    /// # Returns
    ///
    /// A new XhciController instance or an error string.
    pub fn new_with_dma_context(
        mmio_base: usize,
        dma_context: DmaContext,
    ) -> Result<Self, &'static str> {
        // Read capability registers
        let caps = Self::read_capabilities(mmio_base)?;

        println!("[xHCI] Controller found:");
        println!(
            "  Version: {:x}.{:02x}",
            (caps.hci_version >> 8) & 0xFF,
            caps.hci_version & 0xFF
        );
        println!(
            "  Slots: {}, Ports: {}, Interrupters: {}",
            caps.hcs_params1 & 0xFF,
            (caps.hcs_params1 >> 24) & 0xFF,
            (caps.hcs_params1 >> 8) & 0x3FF
        );
        println!(
            "  HCCPARAMS1={:#x} AC64={} CSZ64={}",
            caps.hcc_params1,
            (caps.hcc_params1 & HCC_PARAMS1_AC64) != 0,
            (caps.hcc_params1 & HCC_PARAMS1_CONTEXT_SIZE_64) != 0
        );
        println!("  HCSPARAMS2={:#x}", caps.hcs_params2);

        let regs = RegisterSpace::new(mmio_base, caps.cap_length, caps.rtsoff, caps.dboff);

        let operational = XhciOperational::new(regs.operational_base);

        let max_slots = (caps.hcs_params1 & 0xFF) as u8;
        let max_ports = ((caps.hcs_params1 >> 24) & 0xFF) as u8;
        let max_intrs = ((caps.hcs_params1 >> 8) & 0x3FF) as u16;
        let context_size = if (caps.hcc_params1 & HCC_PARAMS1_CONTEXT_SIZE_64) != 0 {
            64
        } else {
            32
        };

        Ok(Self {
            mmio_base,
            dma_context,
            regs,
            caps,
            operational,
            state: Mutex::new(XhciState::Uninitialized),
            max_slots,
            max_ports,
            max_intrs,
            context_size,
            dcbaa: Mutex::new(None),
            scratchpads: Mutex::new(None),
            cmd_ring: Mutex::new(None),
            event_ring: Mutex::new(None),
            pending_events: Mutex::new(Vec::new()),
            devices: Mutex::new(Vec::new()),
            slot_runtime: Mutex::new(Vec::new()),
            interrupt_id: Mutex::new(None),
        })
    }

    /// Read capability registers from MMIO base
    fn read_capabilities(mmio_base: usize) -> Result<XhciCapabilities, &'static str> {
        unsafe {
            let cap_length =
                core::ptr::read_volatile((mmio_base + capability::CAPLENGTH) as *const u8);
            let hci_version =
                core::ptr::read_volatile((mmio_base + capability::HCIVERSION) as *const u16);
            let hcs_params1 =
                core::ptr::read_volatile((mmio_base + capability::HCSPARAMS1) as *const u32);
            let hcs_params2 =
                core::ptr::read_volatile((mmio_base + capability::HCSPARAMS2) as *const u32);
            let hcs_params3 =
                core::ptr::read_volatile((mmio_base + capability::HCSPARAMS3) as *const u32);
            let hcc_params1 =
                core::ptr::read_volatile((mmio_base + capability::HCCPARAMS1) as *const u32);
            let dboff = core::ptr::read_volatile((mmio_base + capability::DBOFF) as *const u32);
            let rtsoff = core::ptr::read_volatile((mmio_base + capability::RTSOFF) as *const u32);

            Ok(XhciCapabilities {
                cap_length,
                hci_version,
                hcs_params1,
                hcs_params2,
                hcs_params3,
                hcc_params1,
                dboff,
                rtsoff,
            })
        }
    }

    /// Initialize the xHCI controller
    ///
    /// This performs the following steps:
    /// 1. Halt the controller
    /// 2. Reset the controller
    /// 3. Wait for reset to complete
    /// 4. Configure max device slots
    pub fn init(&self) -> Result<(), &'static str> {
        println!("[xHCI] Initializing controller...");

        // Step 1: Halt the controller
        self.halt()?;

        // Step 2: Reset the controller
        self.reset()?;

        // Step 3: Configure max device slots
        let config = self.operational.read_config();
        let max_slots_en = self.max_slots.min(255) as u32;
        self.operational
            .write_config((config & !0xFF) | max_slots_en);
        println!(
            "[xHCI] PAGESIZE={:#x} scratchpads={}",
            self.operational.read_pagesize(),
            self.scratchpad_buffer_count()
        );

        self.setup_dcbaa()?;
        self.setup_command_ring()?;
        self.setup_event_ring()?;

        println!("[xHCI] Configured for {} device slots", max_slots_en);

        *self.state.lock() = XhciState::Halted;
        println!("[xHCI] Controller initialized successfully");

        Ok(())
    }

    /// Halt the xHCI controller
    pub fn halt(&self) -> Result<(), &'static str> {
        let usbcmd = self.operational.read_usbcmd();
        self.operational.write_usbcmd(usbcmd & !0x1);

        let deadline = crate::time::current_time() + 500_000;
        while crate::time::current_time() < deadline {
            let usbsts = self.operational.read_usbsts();
            if (usbsts & 0x1) != 0 {
                println!("[xHCI] Controller halted");
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err("Timeout waiting for xHCI halt")
    }

    /// Reset the xHCI controller
    pub fn reset(&self) -> Result<(), &'static str> {
        let usbcmd = self.operational.read_usbcmd();
        let usbsts = self.operational.read_usbsts();
        println!(
            "[xHCI] Before reset: USBCMD={:#x} USBSTS={:#x}",
            usbcmd, usbsts
        );
        self.operational.write_usbcmd(usbcmd | 0x2);

        let deadline = crate::time::current_time() + 500_000;
        let mut hcrst_cleared = false;
        while crate::time::current_time() < deadline {
            let usbcmd = self.operational.read_usbcmd();
            if (usbcmd & 0x2) == 0 {
                hcrst_cleared = true;
                break;
            }
            core::hint::spin_loop();
        }

        if !hcrst_cleared {
            let usbcmd = self.operational.read_usbcmd();
            let usbsts = self.operational.read_usbsts();
            println!(
                "[xHCI] Reset timeout: USBCMD={:#x} USBSTS={:#x}",
                usbcmd, usbsts
            );
            return Err("Timeout waiting for xHCI reset");
        }

        let deadline = crate::time::current_time() + 500_000;
        while crate::time::current_time() < deadline {
            let usbsts = self.operational.read_usbsts();
            if (usbsts & (1 << 11)) == 0 {
                println!("[xHCI] Controller reset complete");
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err("Timeout waiting for xHCI ready after reset")
    }

    fn scratchpad_buffer_count(&self) -> usize {
        let low = (self.caps.hcs_params2 >> 21) & 0x1f;
        let high = (self.caps.hcs_params2 >> 27) & 0x1f;
        ((high << 5) | low) as usize
    }

    fn dma_map_phys(
        &self,
        paddr: usize,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<usize, &'static str> {
        let dma_addr = self
            .dma_context
            .map_phys(paddr, len, flags)
            .map_err(|_| "xHCI: failed to map DMA buffer")?;
        usize::try_from(dma_addr).map_err(|_| "xHCI: DMA address does not fit usize")
    }

    fn dma_map_pages(
        &self,
        pages: &ContiguousPages,
        flags: IommuMapFlags,
    ) -> Result<usize, &'static str> {
        self.dma_map_phys(
            pages.as_paddr(),
            pages.len() * crate::environment::PAGE_SIZE,
            flags,
        )
    }

    /// Start the xHCI controller
    pub fn start(&self) -> Result<(), &'static str> {
        let stale_status = self.operational.read_usbsts() & USBSTS_WRITE_1_TO_CLEAR;
        if stale_status != 0 {
            println!("[xHCI] Clearing stale USBSTS bits {:#x}", stale_status);
            self.operational.write_usbsts(stale_status);
        }

        let usbcmd = self.operational.read_usbcmd();
        // Set Run/Stop bit (bit 0)
        self.operational.write_usbcmd(usbcmd | 0x1);
        println!(
            "[xHCI] Start command issued: USBCMD={:#x} USBSTS={:#x} CRCR={:#x}",
            self.operational.read_usbcmd(),
            self.operational.read_usbsts(),
            self.operational.read_crcr()
        );

        let deadline = crate::time::current_time() + 500_000;
        while crate::time::current_time() < deadline {
            let usbsts = self.operational.read_usbsts();
            if (usbsts & 0x1) == 0 {
                *self.state.lock() = XhciState::Running;
                println!(
                    "[xHCI] Controller started: USBCMD={:#x} USBSTS={:#x} CRCR={:#x}",
                    self.operational.read_usbcmd(),
                    self.operational.read_usbsts(),
                    self.operational.read_crcr()
                );
                return Ok(());
            }
            core::hint::spin_loop();
        }

        let usbcmd = self.operational.read_usbcmd();
        let usbsts = self.operational.read_usbsts();
        println!(
            "[xHCI] Start timeout: USBCMD={:#x} USBSTS={:#x}",
            usbcmd, usbsts
        );
        Err("Timeout waiting for xHCI start")
    }

    fn setup_scratchpads(&self, count: usize) -> Result<ScratchpadBuffers, &'static str> {
        let array_pages = (count * size_of::<u64>()).div_ceil(crate::environment::PAGE_SIZE);
        let array =
            ContiguousPages::new(array_pages).ok_or("Failed to allocate scratchpad array")?;
        let mut buffers = Vec::new();

        unsafe {
            core::ptr::write_bytes(
                array.as_vaddr() as *mut u8,
                0,
                array_pages * crate::environment::PAGE_SIZE,
            );
        }

        for index in 0..count {
            let buffer = ContiguousPages::new(1).ok_or("Failed to allocate scratchpad buffer")?;
            let buffer_dma_addr = self.dma_map_pages(&buffer, dma_rw_flags())?;
            unsafe {
                let entry = (array.as_vaddr() as *mut u64).add(index);
                write_volatile(entry, buffer_dma_addr as u64);
            }
            buffers.push(buffer);
        }

        let array_dma_addr =
            self.dma_map_pages(&array, IommuMapFlags::READ | IommuMapFlags::COHERENT)?;
        let scratchpads = ScratchpadBuffers {
            array,
            array_dma_addr,
            buffers,
        };
        scratchpads.sync_for_device();
        println!(
            "[xHCI] Scratchpads: count={} array_paddr={:#x} array_dma={:#x} buffer_pages={}",
            count,
            scratchpads.array.as_paddr(),
            scratchpads.array_dma_addr(),
            scratchpads.buffer_count()
        );

        Ok(scratchpads)
    }

    fn setup_dcbaa(&self) -> Result<(), &'static str> {
        let entries = self.max_slots as usize + 1;
        let dcbaa_pages = (entries * size_of::<u64>()).div_ceil(crate::environment::PAGE_SIZE);
        let dcbaa = ContiguousPages::new(dcbaa_pages).ok_or("Failed to allocate DCBAA")?;
        println!("[xHCI] DCBAA paddr={:#x}", dcbaa.as_paddr());
        unsafe {
            core::ptr::write_bytes(
                dcbaa.as_vaddr() as *mut u8,
                0,
                dcbaa_pages * crate::environment::PAGE_SIZE,
            );
        }

        let scratchpads = match self.scratchpad_buffer_count() {
            0 => None,
            count => {
                let scratchpads = self.setup_scratchpads(count)?;
                unsafe {
                    let entries = dcbaa.as_vaddr() as *mut u64;
                    write_volatile(entries, scratchpads.array_dma_addr() as u64);
                }
                Some(scratchpads)
            }
        };

        sync_pages_for_device(&dcbaa);
        let dcbaa_dma_addr =
            self.dma_map_pages(&dcbaa, IommuMapFlags::READ | IommuMapFlags::COHERENT)?;
        self.operational.write_dcbaap(dcbaa_dma_addr as u64);
        println!(
            "[xHCI] DCBAA dma={:#x} DCBAAP readback={:#x}",
            dcbaa_dma_addr,
            self.operational.read_dcbaap()
        );
        *self.scratchpads.lock() = scratchpads;
        *self.dcbaa.lock() = Some(dcbaa);
        Ok(())
    }

    fn setup_command_ring(&self) -> Result<(), &'static str> {
        let ring =
            DmaTrbRing::new_linked(COMMAND_RING_TRBS).ok_or("Failed to allocate command ring")?;
        let ring_dma_addr = self.dma_map_phys(
            ring.physical_address(),
            ring.dma_len(),
            IommuMapFlags::READ | IommuMapFlags::COHERENT,
        )?;
        ring.set_dma_address(ring_dma_addr)?;
        ring.sync_for_device();
        println!(
            "[xHCI] Command ring paddr={:#x} dma={:#x}",
            ring.physical_address(),
            ring.dma_address()
        );
        let crcr = (ring.dma_address() as u64) | u64::from(ring.cycle_state());
        self.operational.write_crcr(crcr);
        println!(
            "[xHCI] CRCR programmed={:#x} readback={:#x}",
            crcr,
            self.operational.read_crcr()
        );
        *self.cmd_ring.lock() = Some(ring);
        Ok(())
    }

    fn setup_event_ring(&self) -> Result<(), &'static str> {
        let ring = EventRing::new(EVENT_RING_TRBS).ok_or("Failed to allocate event ring")?;
        let ring_dma_addr =
            self.dma_map_phys(ring.physical_address(), ring.dma_len(), dma_rw_flags())?;
        let erst_dma_addr = self.dma_map_phys(
            ring.erst_physical_address(),
            ring.erst_dma_len(),
            IommuMapFlags::READ | IommuMapFlags::COHERENT,
        )?;
        ring.set_dma_addresses(ring_dma_addr, erst_dma_addr)?;
        ring.sync_for_device();
        println!(
            "[xHCI] Event ring paddr={:#x} dma={:#x} erst_paddr={:#x} erst_dma={:#x}",
            ring.physical_address(),
            ring.dma_address(),
            ring.erst_physical_address(),
            ring.erst_dma_address()
        );
        unsafe {
            write_volatile(
                (self.regs.runtime_base + registers::runtime::IR0_ERSTSZ) as *mut u32,
                ring.erst_size(),
            );
            write_mmio64_lo_hi(
                self.regs.runtime_base + registers::runtime::IR0_ERSTBA,
                ring.erst_dma_address() as u64,
            );
            write_mmio64_lo_hi(
                self.regs.runtime_base + registers::runtime::IR0_ERDP,
                ring.event_ring_dequeue_pointer() as u64 | ERDP_EVENT_HANDLER_BUSY,
            );
            write_volatile(
                (self.regs.runtime_base + registers::runtime::IR0_IMAN) as *mut u32,
                (1 << 1) | 1, // IE = bit 1, IP(W1C) = bit 0
            );
        }
        println!(
            "[xHCI] Event ring readback: ERSTSZ={:#x} ERSTBA={:#x} ERDP={:#x} IMAN={:#x}",
            self.read_runtime_u32(registers::runtime::IR0_ERSTSZ),
            self.read_runtime_u64(registers::runtime::IR0_ERSTBA),
            self.read_runtime_u64(registers::runtime::IR0_ERDP),
            self.read_runtime_u32(registers::runtime::IR0_IMAN)
        );
        *self.event_ring.lock() = Some(ring);
        Ok(())
    }

    pub fn enable_interrupts(&self, interrupt_id: InterruptId) -> Result<(), &'static str> {
        *self.interrupt_id.lock() = Some(interrupt_id);

        let usbcmd = self.operational.read_usbcmd();
        self.operational.write_usbcmd(usbcmd | (1 << 2)); // INTE = bit 2

        let pending = self.operational.read_usbsts();
        if pending & (USBSTS_EVENT_INTERRUPT | USBSTS_PORT_CHANGE_DETECT) != 0 {
            self.operational
                .write_usbsts(pending & (USBSTS_EVENT_INTERRUPT | USBSTS_PORT_CHANGE_DETECT));
        }

        unsafe {
            write_volatile(
                (self.regs.runtime_base + registers::runtime::IR0_IMAN) as *mut u32,
                (1 << 1) | 1, // IE = bit 1, clear IP = bit 0
            );
        }

        Ok(())
    }

    fn ring_command_doorbell(&self) {
        fence(Ordering::SeqCst);
        unsafe {
            write_volatile(self.regs.doorbell_base as *mut u32, 0);
        }
    }

    fn read_runtime_u32(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.regs.runtime_base + offset) as *const u32) }
    }

    fn read_runtime_u64(&self, offset: usize) -> u64 {
        read_mmio64_lo_hi(self.regs.runtime_base + offset)
    }

    fn log_trb(label: &str, index: usize, trb: Trb) {
        println!(
            "[xHCI] {}[{}]: type={} cycle={} param={:#x} status={:#x} control={:#x}",
            label,
            index,
            trb.trb_type(),
            (trb.control & 1) != 0,
            trb.parameter,
            trb.status,
            trb.control
        );
    }

    fn log_event(label: &str, event: Trb) {
        println!(
            "[xHCI] {}: type={} slot={} ep={} code={} control={:#x} status={:#x} ptr={:#x}",
            label,
            event.trb_type(),
            event.slot_id(),
            event.endpoint_id(),
            event.completion_code(),
            event.control,
            event.status,
            event.trb_pointer()
        );
    }

    fn queue_pending_event(&self, event: Trb) {
        let mut pending = self.pending_events.lock();
        if pending.len() >= XHCI_PENDING_EVENT_LIMIT {
            pending.remove(0);
        }
        pending.push(event);
    }

    fn take_pending_event<F>(&self, mut predicate: F) -> Option<Trb>
    where
        F: FnMut(&Trb) -> bool,
    {
        let mut pending = self.pending_events.lock();
        let index = pending.iter().position(&mut predicate)?;
        Some(pending.remove(index))
    }

    fn take_pending_command_completion(&self) -> Option<Trb> {
        self.take_pending_event(|event| event.trb_type() == TrbType::CommandCompletionEvent as u8)
    }

    fn take_pending_transfer_event(&self, slot_id: u8, endpoint_id: u8) -> Option<Trb> {
        self.take_pending_event(|event| {
            event.trb_type() == TrbType::TransferEvent as u8
                && event.slot_id() == slot_id
                && event.endpoint_id() == endpoint_id
        })
    }

    fn log_command_timeout_state(&self) {
        println!(
            "[xHCI] Command timeout regs: USBCMD={:#x} USBSTS={:#x} CRCR={:#x} DCBAAP={:#x} CONFIG={:#x}",
            self.operational.read_usbcmd(),
            self.operational.read_usbsts(),
            self.operational.read_crcr(),
            self.operational.read_dcbaap(),
            self.operational.read_config()
        );
        println!(
            "[xHCI] Command timeout interrupter: MFINDEX={:#x} IMAN={:#x} IMOD={:#x} ERSTSZ={:#x} ERSTBA={:#x} ERDP={:#x}",
            self.read_runtime_u32(registers::runtime::MFINDEX),
            self.read_runtime_u32(registers::runtime::IR0_IMAN),
            self.read_runtime_u32(registers::runtime::IR0_IMOD),
            self.read_runtime_u32(registers::runtime::IR0_ERSTSZ),
            self.read_runtime_u64(registers::runtime::IR0_ERSTBA),
            self.read_runtime_u64(registers::runtime::IR0_ERDP)
        );

        if let Some(cmd_ring) = self.cmd_ring.lock().as_ref() {
            let capacity = cmd_ring.capacity();
            let producer = cmd_ring.current_producer_index();
            println!(
                "[xHCI] Command ring state: paddr={:#x} capacity={} producer={} cycle={}",
                cmd_ring.physical_address(),
                capacity,
                producer,
                cmd_ring.cycle_state()
            );
            for index in 0..core::cmp::min(capacity, 4) {
                if let Some(trb) = cmd_ring.peek(index) {
                    Self::log_trb("cmd", index, trb);
                }
            }
        } else {
            println!("[xHCI] Command ring state: uninitialized");
        }

        if let Some(event_ring) = self.event_ring.lock().as_ref() {
            let capacity = event_ring.capacity();
            let dequeue = event_ring.current_dequeue_index();
            println!(
                "[xHCI] Event ring state: paddr={:#x} capacity={} dequeue={} cycle={} erdp={:#x}",
                event_ring.physical_address(),
                capacity,
                dequeue,
                event_ring.current_cycle_state(),
                event_ring.event_ring_dequeue_pointer()
            );
            for offset in 0..core::cmp::min(capacity, 4) {
                let index = (dequeue + offset) % capacity;
                if let Some(trb) = event_ring.peek(index) {
                    Self::log_trb("event", index, trb);
                }
            }
        } else {
            println!("[xHCI] Event ring state: uninitialized");
        }

        for port_id in 1..=self.max_ports {
            self.log_port_status(port_id, self.read_portsc(port_id));
        }
    }

    fn send_command(&self, trb: Trb) -> Result<Trb, &'static str> {
        if XHCI_VERBOSE_TRACE {
            println!(
                "[xHCI] Sending command type={} slot={} param={:#x} control={:#x}",
                trb.trb_type(),
                trb.slot_id(),
                trb.parameter,
                trb.control
            );
        }
        {
            let cmd_ring_guard = self.cmd_ring.lock();
            let cmd_ring = cmd_ring_guard
                .as_ref()
                .ok_or("Command ring not initialized")?;
            cmd_ring.enqueue(trb)?;
        }

        if XHCI_VERBOSE_TRACE {
            println!(
                "[xHCI] Before command doorbell: USBCMD={:#x} USBSTS={:#x} CRCR={:#x}",
                self.operational.read_usbcmd(),
                self.operational.read_usbsts(),
                self.operational.read_crcr()
            );
        }
        self.ring_command_doorbell();
        if XHCI_VERBOSE_TRACE {
            println!(
                "[xHCI] After command doorbell: USBCMD={:#x} USBSTS={:#x} CRCR={:#x}",
                self.operational.read_usbcmd(),
                self.operational.read_usbsts(),
                self.operational.read_crcr()
            );
        }
        self.poll_command_completion()
    }

    fn poll_command_completion(&self) -> Result<Trb, &'static str> {
        let deadline = crate::time::current_time() + XHCI_COMMAND_TIMEOUT_US;
        while crate::time::current_time() < deadline {
            if let Some(event) = self.take_pending_command_completion() {
                if event.completion_code() == COMMAND_COMPLETION_SUCCESS {
                    if XHCI_VERBOSE_TRACE {
                        Self::log_event("Command completion event", event);
                    }
                    return Ok(event);
                }
                Self::log_event("Command completion event", event);
                return Err("xHCI command completion failed");
            }

            let event_ring_guard = self.event_ring.lock();
            let event_ring = event_ring_guard
                .as_ref()
                .ok_or("Event ring not initialized")?;
            if let Some(event) = event_ring.dequeue() {
                write_mmio64_lo_hi(
                    self.regs.runtime_base + registers::runtime::IR0_ERDP,
                    event_ring.event_ring_dequeue_pointer() as u64 | ERDP_EVENT_HANDLER_BUSY,
                );
                if event.trb_type() == TrbType::CommandCompletionEvent as u8 {
                    if event.completion_code() == COMMAND_COMPLETION_SUCCESS {
                        if XHCI_VERBOSE_TRACE {
                            Self::log_event("Command completion event", event);
                        }
                        return Ok(event);
                    }
                    Self::log_event("Command completion event", event);
                    return Err("xHCI command completion failed");
                } else {
                    if XHCI_VERBOSE_TRACE {
                        Self::log_event("Event while waiting command", event);
                    }
                    if event.trb_type() == TrbType::TransferEvent as u8 {
                        self.queue_pending_event(event);
                    }
                }
            }
            core::hint::spin_loop();
        }
        self.log_command_timeout_state();
        Err("Timeout waiting for xHCI command completion")
    }

    fn poll_event(&self) -> Option<Trb> {
        let event_ring_guard = self.event_ring.lock();
        let event_ring = event_ring_guard.as_ref()?;
        let event = event_ring.dequeue()?;
        write_mmio64_lo_hi(
            self.regs.runtime_base + registers::runtime::IR0_ERDP,
            event_ring.event_ring_dequeue_pointer() as u64 | ERDP_EVENT_HANDLER_BUSY,
        );
        Some(event)
    }

    fn read_portsc(&self, port_id: u8) -> u32 {
        let offset =
            operational::PORTSC_BASE + ((port_id as usize - 1) * operational::PORT_REGISTER_STRIDE);
        unsafe { read_volatile((self.regs.operational_base + offset) as *const u32) }
    }

    fn write_portsc(&self, port_id: u8, value: u32) {
        let offset =
            operational::PORTSC_BASE + ((port_id as usize - 1) * operational::PORT_REGISTER_STRIDE);
        // SAFETY: Category 11 - provenance-sensitive MMIO access.
        // The controller maps `regs.operational_base` from a live xHCI MMIO region, and
        // `port_id` is only produced from 1..=max_ports. The computed address is inside
        // the operational port register space and must be accessed with volatile writes.
        unsafe {
            write_volatile((self.regs.operational_base + offset) as *mut u32, value);
        }
    }

    fn portsc_write_preserve_bits(portsc: u32) -> u32 {
        portsc & PORTSC_WRITE_PRESERVE_BITS
    }

    fn clear_port_change_bits(&self, port_id: u8, portsc: u32) {
        let status = PortStatus::from_portsc(portsc);
        if status.change_bits == 0 {
            return;
        }
        let clear_value = Self::portsc_write_preserve_bits(portsc) | status.change_bits;
        self.write_portsc(port_id, clear_value);
    }

    fn log_port_status(&self, port_id: u8, portsc: u32) {
        let status = PortStatus::from_portsc(portsc);
        println!(
            "[xHCI] Port {} PORTSC={:#x} ccs={} ped={} pr={} pls={} pp={} speed={} change={:#x} cas={}",
            port_id,
            portsc,
            status.connected,
            status.enabled,
            status.resetting,
            status.link_state,
            status.powered,
            status.speed_raw,
            status.change_bits,
            status.config_error
        );
    }

    fn reset_port(&self, port_id: u8) -> Result<PortStatus, &'static str> {
        let portsc = self.read_portsc(port_id);
        let status = PortStatus::from_portsc(portsc);
        if !status.connected {
            return Err("xHCI port disconnected before reset");
        }
        if status.enabled && !status.resetting {
            return Ok(status);
        }

        self.clear_port_change_bits(port_id, portsc);
        let reset_value = Self::portsc_write_preserve_bits(portsc) | PORTSC_PR;
        println!(
            "[xHCI] Port {} reset start: PORTSC={:#x} write={:#x}",
            port_id, portsc, reset_value
        );
        self.write_portsc(port_id, reset_value);

        let deadline = crate::time::current_time() + PORT_RESET_TIMEOUT_US;
        let mut last_portsc = portsc;
        while crate::time::current_time() < deadline {
            let current_portsc = self.read_portsc(port_id);
            last_portsc = current_portsc;
            let current = PortStatus::from_portsc(current_portsc);
            if !current.connected {
                self.log_port_status(port_id, current_portsc);
                return Err("xHCI port disconnected during reset");
            }
            if !current.resetting && current.enabled {
                self.log_port_status(port_id, current_portsc);
                self.clear_port_change_bits(port_id, current_portsc);
                crate::time::udelay(PORT_RESET_RECOVERY_US);
                return Ok(current);
            }
            core::hint::spin_loop();
        }

        self.log_port_status(port_id, last_portsc);
        Err("Timeout waiting for xHCI port reset")
    }

    fn context_speed(speed: UsbSpeed) -> u8 {
        match speed {
            UsbSpeed::Low => ctx_speed::LOW,
            UsbSpeed::Full => ctx_speed::FULL,
            UsbSpeed::High => ctx_speed::HIGH,
            UsbSpeed::Super | UsbSpeed::SuperPlus => ctx_speed::SUPER,
        }
    }

    fn assign_device_context(&self, slot_id: u8) -> Result<ContiguousPages, &'static str> {
        let pages = ContiguousPages::new(
            device_context_bytes(self.context_size).div_ceil(crate::environment::PAGE_SIZE),
        )
        .ok_or("Failed to allocate device context")?;
        let device_context_dma_addr = self.dma_map_pages(&pages, dma_rw_flags())?;
        unsafe {
            core::ptr::write_bytes(
                pages.as_vaddr() as *mut u8,
                0,
                pages.len() * crate::environment::PAGE_SIZE,
            );
            let dcbaa_guard = self.dcbaa.lock();
            let dcbaa = dcbaa_guard.as_ref().ok_or("DCBAA not initialized")?;
            let entries = dcbaa.as_vaddr() as *mut u64;
            write_volatile(
                entries.add(slot_id as usize),
                device_context_dma_addr as u64,
            );
            sync_pages_for_device(&pages);
            sync_pages_for_device(dcbaa);
        }
        Ok(pages)
    }

    fn allocate_ep0_ring(&self) -> Result<DmaTrbRing, &'static str> {
        let ring = DmaTrbRing::new_linked(64).ok_or("Failed to allocate EP0 transfer ring")?;
        let ring_dma_addr =
            self.dma_map_phys(ring.physical_address(), ring.dma_len(), dma_rw_flags())?;
        ring.set_dma_address(ring_dma_addr)?;
        Ok(ring)
    }

    fn address_device(
        &self,
        slot_id: u8,
        root_port_id: u8,
        speed: UsbSpeed,
        route_string: u32,
        route_depth: u8,
        tt_hub_slot_id: Option<u8>,
        tt_port: u8,
    ) -> Result<SlotRuntime, &'static str> {
        let device_context = self.assign_device_context(slot_id)?;
        let input_pages = ContiguousPages::new(
            address_input_context_bytes(self.context_size).div_ceil(crate::environment::PAGE_SIZE),
        )
        .ok_or("Failed to allocate input context")?;
        let ep0_ring = self.allocate_ep0_ring()?;
        ep0_ring.clear();
        let input_dma_addr =
            self.dma_map_pages(&input_pages, IommuMapFlags::READ | IommuMapFlags::COHERENT)?;
        unsafe {
            core::ptr::write_bytes(
                input_pages.as_vaddr() as *mut u8,
                0,
                input_pages.len() * crate::environment::PAGE_SIZE,
            );
            let input = InputContextBuffer::new(input_pages.as_vaddr(), self.context_size);
            let mut address_context = InputContext::new();
            address_context.configure_for_address(
                slot_id,
                root_port_id,
                Self::context_speed(speed),
                route_string,
                tt_hub_slot_id,
                tt_port,
            );
            address_context
                .endpoint0
                .set_dequeue_pointer(ep0_ring.dma_address() as u64);
            address_context.endpoint0.set_dequeue_cycle(true);
            core::ptr::write(input.control_mut(), address_context.control);
            core::ptr::write(input.slot_mut(), address_context.slot);
            core::ptr::write(input.endpoint_mut(EP0_DCI)?, address_context.endpoint0);
        }
        sync_pages_for_device(&input_pages);
        ep0_ring.sync_for_device();
        let event = self.send_command(Trb::address_device_command(
            input_dma_addr as u64,
            slot_id,
            false,
        ))?;
        if event.slot_id() != slot_id {
            return Err("Address Device completion slot mismatch");
        }
        sync_pages_after_device_write(&device_context);
        let mut usb_device = UsbDevice::new(slot_id, root_port_id, speed);
        usb_device.set_state(UsbDeviceState::Default);
        usb_device.assign_address(slot_id);
        Ok(SlotRuntime {
            usb_device,
            route_string,
            route_depth,
            device_context,
            ep0_input_context: input_pages,
            ep0_ring,
            interrupt_input_context: None,
            interrupt_ring: None,
            interrupt_buffer: None,
            interrupt_dci: None,
            interrupt_endpoint_address: None,
            interrupt_max_packet_size: None,
            storage: None,
            hid: None,
            hub: None,
        })
    }

    fn ring_endpoint_doorbell(&self, slot_id: u8, endpoint_id: u8) {
        fence(Ordering::SeqCst);
        let offset = (slot_id as usize) * size_of::<u32>();
        let doorbell = (self.regs.doorbell_base + offset) as *mut u32;
        unsafe {
            write_volatile(doorbell, endpoint_id as u32);
            let _ = read_volatile(doorbell);
        }
    }

    fn transfer_successful(event: Trb) -> bool {
        matches!(
            event.completion_code(),
            COMMAND_COMPLETION_SUCCESS | TRANSFER_EVENT_SHORT_PACKET
        )
    }

    fn control_transfer(
        &self,
        slot: &SlotRuntime,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        mut data: Option<&mut ContiguousPages>,
        length: u16,
    ) -> Result<Trb, &'static str> {
        println!(
            "[xHCI] EP0 control transfer: slot={} req_type={:#x} req={:#x} value={:#x} index={:#x} length={} data={}",
            slot.usb_device.slot_id(),
            request_type,
            request,
            value,
            index,
            length,
            data.is_some()
        );
        let direction_in = (request_type & 0x80) != 0;
        let transfer_type = if data.is_some() { 3 } else { 0 };
        let required_trbs = if data.is_some() { 3 } else { 2 };
        slot.ep0_ring
            .ensure_contiguous_space(required_trbs, Trb::no_op_transfer())?;
        slot.ep0_ring.enqueue(Trb::setup_stage(
            request_type,
            request,
            value,
            index,
            length,
            transfer_type,
        ))?;
        if let Some(buffer) = data.as_mut() {
            let flags = if direction_in {
                IommuMapFlags::WRITE | IommuMapFlags::COHERENT
            } else {
                IommuMapFlags::READ | IommuMapFlags::COHERENT
            };
            if direction_in {
                sync_pages_before_device_write(buffer);
            } else {
                sync_pages_for_device(buffer);
            }
            let buffer_dma_addr = self.dma_map_pages(buffer, flags)?;
            slot.ep0_ring.enqueue(Trb::data_stage(
                buffer_dma_addr as u64,
                length as u32,
                direction_in,
            ))?;
            slot.ep0_ring
                .enqueue(Trb::status_stage(!direction_in, true))?;
        } else {
            slot.ep0_ring.enqueue(Trb::status_stage(true, true))?;
        }

        self.ring_endpoint_doorbell(slot.usb_device.slot_id(), EP0_DCI);
        let event = self.wait_for_transfer_event(slot.usb_device.slot_id(), EP0_DCI)?;
        if direction_in {
            if let Some(buffer) = data {
                sync_pages_after_device_write(buffer);
            }
        }
        Ok(event)
    }

    fn wait_for_transfer_event(&self, slot_id: u8, endpoint_id: u8) -> Result<Trb, &'static str> {
        let deadline = crate::time::current_time() + XHCI_TRANSFER_TIMEOUT_US;
        while crate::time::current_time() < deadline {
            if let Some(event) = self.take_pending_transfer_event(slot_id, endpoint_id) {
                if Self::transfer_successful(event) {
                    if XHCI_VERBOSE_TRACE {
                        Self::log_event("Transfer event", event);
                    }
                    return Ok(event);
                }
                Self::log_event("Transfer event", event);
                return Err("xHCI transfer event failed");
            }

            if let Some(event) = self.poll_event() {
                match event.trb_type() {
                    value if value == TrbType::TransferEvent as u8 => {
                        if event.slot_id() == slot_id && event.endpoint_id() == endpoint_id {
                            if Self::transfer_successful(event) {
                                if XHCI_VERBOSE_TRACE {
                                    Self::log_event("Transfer event", event);
                                }
                                return Ok(event);
                            }
                            Self::log_event("Transfer event", event);
                            return Err("xHCI transfer event failed");
                        } else if XHCI_VERBOSE_TRACE {
                            Self::log_event("Event while waiting", event);
                            self.queue_pending_event(event);
                        } else {
                            self.queue_pending_event(event);
                        }
                    }
                    value if value == TrbType::PortStatusChangeEvent as u8 => {
                        if XHCI_VERBOSE_TRACE {
                            Self::log_event("Event while waiting", event);
                        }
                        self.handle_port_change_detected();
                    }
                    value if value == TrbType::CommandCompletionEvent as u8 => {
                        self.queue_pending_event(event);
                    }
                    _ if XHCI_VERBOSE_TRACE => {
                        Self::log_event("Event while waiting", event);
                    }
                    _ => {}
                }
            }
            core::hint::spin_loop();
        }
        self.log_transfer_timeout_state(slot_id, endpoint_id);
        Err("Timeout waiting for transfer event")
    }

    fn log_transfer_timeout_state(&self, slot_id: u8, endpoint_id: u8) {
        println!(
            "[xHCI] Transfer wait timeout: slot={} ep={}",
            slot_id, endpoint_id
        );

        if let Some(event_ring) = self.event_ring.lock().as_ref() {
            let capacity = event_ring.capacity();
            let dequeue = event_ring.current_dequeue_index();
            println!(
                "[xHCI] Event ring state: paddr={:#x} capacity={} dequeue={} cycle={} erdp={:#x}",
                event_ring.physical_address(),
                capacity,
                dequeue,
                event_ring.current_cycle_state(),
                event_ring.event_ring_dequeue_pointer()
            );
            for offset in 0..core::cmp::min(capacity, 4) {
                let index = (dequeue + offset) % capacity;
                if let Some(trb) = event_ring.peek(index) {
                    Self::log_trb("event", index, trb);
                }
            }
        }
        self.log_bulk_endpoint_timeout_state(slot_id, endpoint_id);
    }

    fn log_bulk_endpoint_timeout_state(&self, slot_id: u8, endpoint_id: u8) {
        let slots = self.slot_runtime.lock();
        let Some(slot) = slots
            .iter()
            .find(|slot| slot.usb_device.slot_id() == slot_id)
        else {
            return;
        };
        let Some(storage) = slot.storage.as_ref() else {
            return;
        };

        sync_pages_after_device_write(&slot.device_context);
        let context = DeviceContextBuffer::new(slot.device_context.as_vaddr(), self.context_size);
        if let Ok(endpoint) = context.endpoint(endpoint_id) {
            let endpoint = unsafe { read_volatile(endpoint) };
            println!(
                "[xHCI] Endpoint context slot={} dci={}: d0={:#x} d1={:#x} trdp={:#x}:{:#x} d4={:#x}",
                slot_id,
                endpoint_id,
                endpoint.dword0,
                endpoint.dword1,
                endpoint.tr_dequeue_high,
                endpoint.tr_dequeue_low,
                endpoint.dword4
            );
        }

        let (label, endpoint, ring) = if storage.bulk_in.dci == endpoint_id {
            ("bulk-in", &storage.bulk_in, &storage.bulk_in.ring)
        } else if storage.bulk_out.dci == endpoint_id {
            ("bulk-out", &storage.bulk_out, &storage.bulk_out.ring)
        } else {
            return;
        };
        println!(
            "[xHCI] {} ring slot={} ep_addr={:#x} dci={} max_packet={} paddr={:#x} dma={:#x} producer={} cycle={}",
            label,
            slot_id,
            endpoint.endpoint_address,
            endpoint.dci,
            endpoint.max_packet_size,
            ring.physical_address(),
            ring.dma_address(),
            ring.current_producer_index(),
            ring.cycle_state()
        );
        for index in 0..core::cmp::min(ring.capacity(), 4) {
            if let Some(trb) = ring.peek(index) {
                Self::log_trb(label, index, trb);
            }
        }
    }

    fn submit_interrupt_in_transfer(&self, slot_id: u8) -> Result<(), &'static str> {
        let slots = self.slot_runtime.lock();
        let slot = slots
            .iter()
            .find(|slot| slot.usb_device.slot_id() == slot_id)
            .ok_or("Unknown slot for interrupt transfer")?;
        let ring = slot
            .interrupt_ring
            .as_ref()
            .ok_or("Interrupt ring not configured")?;
        let buffer = slot
            .interrupt_buffer
            .as_ref()
            .ok_or("Interrupt buffer not configured")?;
        let dci = slot.interrupt_dci.ok_or("Interrupt DCI not configured")?;
        let max_packet = slot
            .interrupt_max_packet_size
            .ok_or("Interrupt packet size not configured")?;

        let buffer_dma_addr =
            self.dma_map_pages(buffer, IommuMapFlags::WRITE | IommuMapFlags::COHERENT)?;
        ring.enqueue(Trb::normal_transfer_in(
            buffer_dma_addr as u64,
            max_packet as u32,
        ))?;
        self.ring_endpoint_doorbell(slot_id, dci);
        Ok(())
    }

    fn configure_boot_hid_slot(&self, slot_id: u8) -> Result<bool, &'static str> {
        println!("[xHCI] Configuring boot HID for slot {}", slot_id);
        let descriptor = {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for descriptor fetch")?;
            self.get_device_descriptor(slot)?
        };

        let config_blob = {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for config fetch")?;

            let mut header_buffer =
                ContiguousPages::new(1).ok_or("Failed to allocate config header buffer")?;
            unsafe {
                core::ptr::write_bytes(
                    header_buffer.as_vaddr() as *mut u8,
                    0,
                    crate::environment::PAGE_SIZE,
                )
            };
            self.control_transfer(
                slot,
                0x80,
                USB_REQ_GET_DESCRIPTOR,
                (USB_DT_CONFIGURATION as u16) << 8,
                0,
                Some(&mut header_buffer),
                ConfigurationDescriptor::encoded_size() as u16,
            )?;
            let header = unsafe {
                read_unaligned(header_buffer.as_vaddr() as *const ConfigurationDescriptor)
            };
            self.get_configuration_blob(slot, header.total_length)?
        };

        let boot = match self.parse_boot_interface(&config_blob) {
            Ok(boot) => boot,
            Err(_) => return Ok(false),
        };

        println!(
            "[xHCI] Slot {} boot interface: protocol={:?} ep={:#x} max_packet={} interval={} cfg={} if={}",
            slot_id,
            boot.protocol,
            boot.endpoint_address,
            boot.max_packet_size,
            boot.interval,
            boot.configuration_value,
            boot.interface_number
        );

        let _ = descriptor;
        let interrupt_dci = Self::interrupt_dci(boot.endpoint_address);
        let interrupt_ring =
            DmaTrbRing::new_linked(64).ok_or("Failed to allocate interrupt ring")?;
        let interrupt_ring_dma_addr = self.dma_map_phys(
            interrupt_ring.physical_address(),
            interrupt_ring.dma_len(),
            dma_rw_flags(),
        )?;
        interrupt_ring.set_dma_address(interrupt_ring_dma_addr)?;
        let interrupt_buffer_pages = match boot.protocol {
            HidBootProtocol::Keyboard => {
                KeyboardBootReport::encoded_size().div_ceil(crate::environment::PAGE_SIZE)
            }
            HidBootProtocol::Mouse => {
                MouseBootReport::encoded_size().div_ceil(crate::environment::PAGE_SIZE)
            }
        };
        let interrupt_buffer = ContiguousPages::new(interrupt_buffer_pages)
            .ok_or("Failed to allocate interrupt buffer")?;
        let input_pages = ContiguousPages::new(
            full_input_context_bytes(self.context_size).div_ceil(crate::environment::PAGE_SIZE),
        )
        .ok_or("Failed to allocate endpoint config context")?;
        let input_dma_addr =
            self.dma_map_pages(&input_pages, IommuMapFlags::READ | IommuMapFlags::COHERENT)?;

        unsafe {
            core::ptr::write_bytes(
                input_pages.as_vaddr() as *mut u8,
                0,
                input_pages.len() * crate::environment::PAGE_SIZE,
            );
            let input = InputContextBuffer::new(input_pages.as_vaddr(), self.context_size);
            let control = &mut *input.control_mut();
            control.add_slot_context();
            control.add_endpoint(interrupt_dci);
            control.configuration_value = boot.configuration_value as u32;
            control.alternate_settings = boot.interface_number as u32;

            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for endpoint config")?;
            sync_pages_after_device_write(&slot.device_context);
            let device_speed = slot.usb_device.speed();
            let existing_ctx =
                DeviceContextBuffer::new(slot.device_context.as_vaddr(), self.context_size);
            let mut slot_ctx = core::ptr::read(existing_ctx.slot());
            slot_ctx.set_context_entries(interrupt_dci);
            core::ptr::write(input.slot_mut(), slot_ctx);
            let xhci_interval = Self::xhci_interval(device_speed, boot.interval);
            let (_, ep_ctx) = InputContext::interrupt_endpoint_context(
                interrupt_dci,
                boot.max_packet_size,
                xhci_interval,
                interrupt_ring.dma_address() as u64,
                true,
            );
            core::ptr::write(input.endpoint_mut(interrupt_dci)?, ep_ctx);
        }
        sync_pages_for_device(&input_pages);
        interrupt_ring.sync_for_device();

        let event = self.send_command(Trb::configure_endpoint_command(
            input_dma_addr as u64,
            slot_id,
            false,
        ))?;
        if event.slot_id() != slot_id {
            return Err("Configure Endpoint completion slot mismatch");
        }

        {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for set configuration")?;
            self.control_transfer(
                slot,
                0x00,
                USB_REQ_SET_CONFIGURATION,
                boot.configuration_value as u16,
                0,
                None,
                0,
            )?;
            self.control_transfer(
                slot,
                0x21,
                USB_REQ_SET_PROTOCOL,
                0,
                boot.interface_number as u16,
                None,
                0,
            )?;
        }

        match boot.protocol {
            HidBootProtocol::Keyboard => self.attach_boot_keyboard(slot_id)?,
            HidBootProtocol::Mouse => self.attach_boot_mouse(slot_id)?,
        }

        {
            let mut slots = self.slot_runtime.lock();
            let slot = slots
                .iter_mut()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for HID runtime update")?;
            slot.usb_device.set_state(UsbDeviceState::Configured);
            slot.interrupt_input_context = Some(input_pages);
            slot.interrupt_ring = Some(interrupt_ring);
            slot.interrupt_buffer = Some(interrupt_buffer);
            slot.interrupt_dci = Some(interrupt_dci);
            slot.interrupt_endpoint_address = Some(boot.endpoint_address);
            slot.interrupt_max_packet_size = Some(boot.max_packet_size);
        }

        self.submit_interrupt_in_transfer(slot_id)?;

        Ok(true)
    }

    fn configure_mass_storage_slot(self: &Arc<Self>, slot_id: u8) -> Result<bool, &'static str> {
        {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for mass storage setup")?;
            if slot.storage.is_some() {
                return Ok(true);
            }
        }

        let config_blob = {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for storage config fetch")?;

            let mut header_buffer =
                ContiguousPages::new(1).ok_or("Failed to allocate config header buffer")?;
            unsafe {
                core::ptr::write_bytes(
                    header_buffer.as_vaddr() as *mut u8,
                    0,
                    crate::environment::PAGE_SIZE,
                )
            };
            self.control_transfer(
                slot,
                0x80,
                USB_REQ_GET_DESCRIPTOR,
                (USB_DT_CONFIGURATION as u16) << 8,
                0,
                Some(&mut header_buffer),
                ConfigurationDescriptor::encoded_size() as u16,
            )?;
            let header = unsafe {
                read_unaligned(header_buffer.as_vaddr() as *const ConfigurationDescriptor)
            };
            self.get_configuration_blob(slot, header.total_length)?
        };

        let storage = match self.parse_mass_storage_interface(&config_blob) {
            Ok(storage) => storage,
            Err(_) => return Ok(false),
        };

        println!(
            "[xHCI] Slot {} mass storage interface: cfg={} if={} bulk_in={:#x}/{} bulk_out={:#x}/{}",
            slot_id,
            storage.configuration_value,
            storage.interface_number,
            storage.bulk_in_endpoint,
            storage.bulk_in_max_packet_size,
            storage.bulk_out_endpoint,
            storage.bulk_out_max_packet_size
        );

        let bulk_in_dci = Self::endpoint_dci(storage.bulk_in_endpoint);
        let bulk_out_dci = Self::endpoint_dci(storage.bulk_out_endpoint);
        let max_dci = bulk_in_dci.max(bulk_out_dci);
        let bulk_in_ring = DmaTrbRing::new_linked(64).ok_or("Failed to allocate bulk IN ring")?;
        let bulk_out_ring = DmaTrbRing::new_linked(64).ok_or("Failed to allocate bulk OUT ring")?;
        let bulk_in_dma_addr = self.dma_map_phys(
            bulk_in_ring.physical_address(),
            bulk_in_ring.dma_len(),
            dma_rw_flags(),
        )?;
        bulk_in_ring.set_dma_address(bulk_in_dma_addr)?;
        let bulk_out_dma_addr = self.dma_map_phys(
            bulk_out_ring.physical_address(),
            bulk_out_ring.dma_len(),
            dma_rw_flags(),
        )?;
        bulk_out_ring.set_dma_address(bulk_out_dma_addr)?;
        let input_pages = ContiguousPages::new(
            full_input_context_bytes(self.context_size).div_ceil(crate::environment::PAGE_SIZE),
        )
        .ok_or("Failed to allocate mass storage endpoint context")?;
        let input_dma_addr =
            self.dma_map_pages(&input_pages, IommuMapFlags::READ | IommuMapFlags::COHERENT)?;

        unsafe {
            core::ptr::write_bytes(
                input_pages.as_vaddr() as *mut u8,
                0,
                input_pages.len() * crate::environment::PAGE_SIZE,
            );
            let input = InputContextBuffer::new(input_pages.as_vaddr(), self.context_size);
            let control = &mut *input.control_mut();
            control.add_slot_context();
            control.add_endpoint(bulk_in_dci);
            control.add_endpoint(bulk_out_dci);
            control.configuration_value = storage.configuration_value as u32;
            control.alternate_settings = storage.interface_number as u32;

            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for mass storage endpoint config")?;
            sync_pages_after_device_write(&slot.device_context);
            let existing_ctx =
                DeviceContextBuffer::new(slot.device_context.as_vaddr(), self.context_size);
            let mut slot_ctx = core::ptr::read(existing_ctx.slot());
            slot_ctx.set_context_entries(max_dci);
            core::ptr::write(input.slot_mut(), slot_ctx);

            let (_, bulk_in_ctx) = InputContext::bulk_endpoint_context(
                bulk_in_dci,
                storage.bulk_in_max_packet_size,
                bulk_in_ring.dma_address() as u64,
                true,
            );
            let (_, bulk_out_ctx) = InputContext::bulk_endpoint_context(
                bulk_out_dci,
                storage.bulk_out_max_packet_size,
                bulk_out_ring.dma_address() as u64,
                false,
            );
            core::ptr::write(input.endpoint_mut(bulk_in_dci)?, bulk_in_ctx);
            core::ptr::write(input.endpoint_mut(bulk_out_dci)?, bulk_out_ctx);
        }
        sync_pages_for_device(&input_pages);
        bulk_in_ring.sync_for_device();
        bulk_out_ring.sync_for_device();

        let event = self.send_command(Trb::configure_endpoint_command(
            input_dma_addr as u64,
            slot_id,
            false,
        ))?;
        if event.slot_id() != slot_id {
            return Err("Configure Endpoint completion slot mismatch");
        }

        {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for mass storage set configuration")?;
            self.control_transfer(
                slot,
                0x00,
                USB_REQ_SET_CONFIGURATION,
                storage.configuration_value as u16,
                0,
                None,
                0,
            )?;
        }

        {
            let mut slots = self.slot_runtime.lock();
            let slot = slots
                .iter_mut()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for mass storage runtime update")?;
            slot.usb_device.set_state(UsbDeviceState::Configured);
            slot.storage = Some(MassStorageRuntime {
                input_context: input_pages,
                bulk_in: BulkEndpointRuntime {
                    endpoint_address: storage.bulk_in_endpoint,
                    dci: bulk_in_dci,
                    max_packet_size: storage.bulk_in_max_packet_size,
                    ring: bulk_in_ring,
                },
                bulk_out: BulkEndpointRuntime {
                    endpoint_address: storage.bulk_out_endpoint,
                    dci: bulk_out_dci,
                    max_packet_size: storage.bulk_out_max_packet_size,
                    ring: bulk_out_ring,
                },
            });
        }

        let block_device = Arc::new(UsbMassStorageBlockDevice::new(
            self.clone(),
            slot_id,
            storage.bulk_in_endpoint,
            storage.bulk_out_endpoint,
        ));
        block_device.initialize()?;
        let name = next_usb_block_device_name();
        DeviceManager::get_manager().register_device_with_name(name.clone(), block_device);
        println!("[usb-storage] registered {}", name);

        Ok(true)
    }

    fn configure_known_classes(&self, slot_id: u8) {
        match self.configure_boot_hid_slot(slot_id) {
            Ok(true) => {
                println!("[xHCI] Slot {} boot HID configured", slot_id);
                return;
            }
            Ok(false) => println!("[xHCI] Slot {} is not a boot HID device", slot_id),
            Err(error) => println!("[xHCI] Slot {} HID setup failed: {}", slot_id, error),
        }

        match self.configure_hub_slot(slot_id) {
            Ok(true) => println!("[xHCI] Slot {} hub configured", slot_id),
            Ok(false) => {}
            Err(error) => println!("[xHCI] Slot {} hub setup failed: {}", slot_id, error),
        }
    }

    fn configure_hub_slot(&self, slot_id: u8) -> Result<bool, &'static str> {
        {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for hub setup")?;
            if slot.hub.is_some() {
                return Ok(true);
            }
        }

        let descriptor = {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for hub descriptor fetch")?;
            self.get_device_descriptor(slot)?
        };
        let config_blob = {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for hub config fetch")?;
            self.fetch_configuration_blob(slot)?
        };

        let device_class = descriptor.device_class;
        let device_protocol = descriptor.device_protocol;
        let hub_interface = match self.parse_hub_interface(&config_blob) {
            Ok(interface) => interface,
            Err(error) if device_class == USB_CLASS_HUB => {
                println!(
                    "[xHCI] Slot {} hub interface not found in config: {}",
                    slot_id, error
                );
                return Err(error);
            }
            Err(_) => return Ok(false),
        };

        {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for hub set configuration")?;
            self.control_transfer(
                slot,
                0x00,
                USB_REQ_SET_CONFIGURATION,
                hub_interface.configuration_value as u16,
                0,
                None,
                0,
            )?;
            if hub_interface.alternate_setting != 0 {
                self.control_transfer(
                    slot,
                    0x01,
                    USB_REQ_SET_INTERFACE,
                    hub_interface.alternate_setting as u16,
                    hub_interface.interface_number as u16,
                    None,
                    0,
                )?;
            }
        }

        let hub_descriptor = {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for hub descriptor fetch")?;
            self.get_hub_descriptor(slot)?
        };
        let num_ports = hub_descriptor.num_ports;
        if num_ports == 0 {
            return Err("Hub reports zero downstream ports");
        }
        let multi_tt = device_protocol == 2 || hub_interface.protocol == 2;
        let power_good_time_us =
            (hub_descriptor.power_on_to_power_good as u64 * 2_000).max(USB_HUB_POWER_RECOVERY_US);

        println!(
            "[xHCI] Slot {} hub: ports={} multi_tt={} power_good_us={} cfg={} if={} alt={}",
            slot_id,
            num_ports,
            multi_tt,
            power_good_time_us,
            hub_interface.configuration_value,
            hub_interface.interface_number,
            hub_interface.alternate_setting
        );

        self.configure_hub_context(slot_id, num_ports, multi_tt)?;

        {
            let mut slots = self.slot_runtime.lock();
            let slot = slots
                .iter_mut()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for hub runtime update")?;
            slot.usb_device.set_state(UsbDeviceState::Configured);
            slot.hub = Some(HubRuntime {
                route_string: slot.route_string,
                depth: slot.route_depth,
                num_ports,
                multi_tt,
                power_good_time_us,
            });
        }

        self.power_hub_ports(slot_id)?;
        let discovered = self.enumerate_hub_ports(slot_id)?;
        if discovered != 0 {
            println!(
                "[xHCI] Slot {} hub enumerated {} downstream device(s)",
                slot_id, discovered
            );
        }

        Ok(true)
    }

    fn configure_hub_context(
        &self,
        slot_id: u8,
        num_ports: u8,
        multi_tt: bool,
    ) -> Result<(), &'static str> {
        let input_pages = ContiguousPages::new(
            full_input_context_bytes(self.context_size).div_ceil(crate::environment::PAGE_SIZE),
        )
        .ok_or("Failed to allocate hub context")?;
        let input_dma_addr =
            self.dma_map_pages(&input_pages, IommuMapFlags::READ | IommuMapFlags::COHERENT)?;

        unsafe {
            core::ptr::write_bytes(
                input_pages.as_vaddr() as *mut u8,
                0,
                input_pages.len() * crate::environment::PAGE_SIZE,
            );
            let input = InputContextBuffer::new(input_pages.as_vaddr(), self.context_size);
            let control = &mut *input.control_mut();
            control.add_slot_context();

            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for hub context config")?;
            sync_pages_after_device_write(&slot.device_context);
            let existing_ctx =
                DeviceContextBuffer::new(slot.device_context.as_vaddr(), self.context_size);
            let mut slot_ctx = core::ptr::read(existing_ctx.slot());
            slot_ctx.set_hub(true);
            slot_ctx.set_multi_tt(multi_tt);
            slot_ctx.set_num_ports(num_ports);
            slot_ctx.set_context_entries(1);
            core::ptr::write(input.slot_mut(), slot_ctx);
        }
        sync_pages_for_device(&input_pages);

        let event = self.send_command(Trb::configure_endpoint_command(
            input_dma_addr as u64,
            slot_id,
            false,
        ))?;
        if event.slot_id() != slot_id {
            return Err("Configure Endpoint completion slot mismatch");
        }

        Ok(())
    }

    fn power_hub_ports(&self, slot_id: u8) -> Result<(), &'static str> {
        let (num_ports, power_good_time_us) = {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown hub slot")?;
            let hub = slot.hub.ok_or("Slot is not a hub")?;
            (hub.num_ports, hub.power_good_time_us)
        };

        for port in 1..=num_ports {
            self.hub_set_port_feature(slot_id, port, USB_HUB_FEATURE_PORT_POWER)?;
        }
        crate::time::udelay(power_good_time_us);
        Ok(())
    }

    fn enumerate_hub_ports(&self, slot_id: u8) -> Result<usize, &'static str> {
        let (hub, root_port_id, parent_speed) = {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown hub slot")?;
            (
                slot.hub.ok_or("Slot is not a hub")?,
                slot.usb_device.port_id(),
                slot.usb_device.speed(),
            )
        };
        let mut discovered = 0usize;

        for port in 1..=hub.num_ports {
            let _ = self.hub_clear_port_feature(slot_id, port, USB_HUB_FEATURE_C_PORT_CONNECTION);
            let _ = self.hub_clear_port_feature(slot_id, port, USB_HUB_FEATURE_C_PORT_ENABLE);

            let status = self.hub_get_port_status(slot_id, port)?;
            self.log_hub_port_status(slot_id, port, status);
            if !status.connected() {
                continue;
            }
            if hub.depth >= USB_HUB_ROUTE_DEPTH_MAX || port > 15 {
                println!(
                    "[xHCI] Slot {} hub port {} route depth unsupported",
                    slot_id, port
                );
                continue;
            }

            let status = self.reset_hub_port(slot_id, port)?;
            self.log_hub_port_status(slot_id, port, status);
            if !status.enabled() {
                println!(
                    "[xHCI] Slot {} hub port {} reset did not enable port",
                    slot_id, port
                );
                continue;
            }

            let child_route = hub.route_string | ((port as u32) << (hub.depth as u32 * 4));
            let child_depth = hub.depth + 1;
            if self.slot_runtime.lock().iter().any(|slot| {
                slot.usb_device.port_id() == root_port_id
                    && slot.route_string == child_route
                    && slot.route_depth == child_depth
            }) {
                continue;
            }

            let speed = status.speed();
            let tt_hub_slot_id = if matches!(parent_speed, UsbSpeed::High)
                && matches!(speed, UsbSpeed::Low | UsbSpeed::Full)
            {
                Some(slot_id)
            } else {
                None
            };

            println!(
                "[xHCI] Slot {} hub port {} connected, speed {:?}, route={:#x}",
                slot_id, port, speed, child_route
            );
            let completion = self.send_command(Trb::enable_slot_command())?;
            let child_slot_id = completion.slot_id();
            if child_slot_id == 0 {
                return Err("Enable Slot returned slot 0");
            }
            let slot_runtime = self.address_device(
                child_slot_id,
                root_port_id,
                speed,
                child_route,
                child_depth,
                tt_hub_slot_id,
                port,
            )?;
            self.devices.lock().push(slot_runtime.usb_device);
            self.slot_runtime.lock().push(slot_runtime);
            self.configure_known_classes(child_slot_id);
            discovered += 1;
        }

        Ok(discovered)
    }

    fn reset_hub_port(&self, slot_id: u8, port: u8) -> Result<HubPortStatus, &'static str> {
        self.hub_set_port_feature(slot_id, port, USB_HUB_FEATURE_PORT_RESET)?;
        let deadline = crate::time::current_time() + USB_HUB_PORT_RESET_TIMEOUT_US;
        let mut last_status = self.hub_get_port_status(slot_id, port)?;
        while crate::time::current_time() < deadline {
            let status = self.hub_get_port_status(slot_id, port)?;
            last_status = status;
            if status.reset_complete_changed() {
                self.hub_clear_port_feature(slot_id, port, USB_HUB_FEATURE_C_PORT_RESET)?;
                crate::time::udelay(PORT_RESET_RECOVERY_US);
                return Ok(status);
            }
            core::hint::spin_loop();
        }

        self.log_hub_port_status(slot_id, port, last_status);
        Err("Timeout waiting for hub port reset")
    }

    fn hub_set_port_feature(
        &self,
        slot_id: u8,
        port: u8,
        feature: u16,
    ) -> Result<(), &'static str> {
        let slots = self.slot_runtime.lock();
        let slot = slots
            .iter()
            .find(|slot| slot.usb_device.slot_id() == slot_id)
            .ok_or("Unknown hub slot")?;
        self.control_transfer(
            slot,
            0x23,
            USB_REQ_SET_FEATURE,
            feature,
            port as u16,
            None,
            0,
        )?;
        Ok(())
    }

    fn hub_clear_port_feature(
        &self,
        slot_id: u8,
        port: u8,
        feature: u16,
    ) -> Result<(), &'static str> {
        let slots = self.slot_runtime.lock();
        let slot = slots
            .iter()
            .find(|slot| slot.usb_device.slot_id() == slot_id)
            .ok_or("Unknown hub slot")?;
        self.control_transfer(
            slot,
            0x23,
            USB_REQ_CLEAR_FEATURE,
            feature,
            port as u16,
            None,
            0,
        )?;
        Ok(())
    }

    fn hub_get_port_status(&self, slot_id: u8, port: u8) -> Result<HubPortStatus, &'static str> {
        let mut buffer =
            ContiguousPages::new(1).ok_or("Failed to allocate hub port status buffer")?;
        unsafe {
            core::ptr::write_bytes(
                buffer.as_vaddr() as *mut u8,
                0,
                crate::environment::PAGE_SIZE,
            )
        };
        {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown hub slot")?;
            self.control_transfer(
                slot,
                0xa3,
                USB_REQ_GET_STATUS,
                0,
                port as u16,
                Some(&mut buffer),
                4,
            )?;
        }

        let bytes = unsafe { core::slice::from_raw_parts(buffer.as_vaddr() as *const u8, 4) };
        Ok(HubPortStatus {
            status: u16::from_le_bytes([bytes[0], bytes[1]]),
            change: u16::from_le_bytes([bytes[2], bytes[3]]),
        })
    }

    fn log_hub_port_status(&self, slot_id: u8, port: u8, status: HubPortStatus) {
        println!(
            "[xHCI] Slot {} hub port {} status={:#06x} change={:#06x} connected={} enabled={} c_connection={} c_reset={} speed={:?}",
            slot_id,
            port,
            status.status,
            status.change,
            status.connected(),
            status.enabled(),
            status.connection_changed(),
            status.reset_complete_changed(),
            status.speed()
        );
    }

    fn get_device_descriptor(&self, slot: &SlotRuntime) -> Result<DeviceDescriptor, &'static str> {
        let mut buffer =
            ContiguousPages::new(1).ok_or("Failed to allocate device descriptor buffer")?;
        unsafe {
            core::ptr::write_bytes(
                buffer.as_vaddr() as *mut u8,
                0,
                crate::environment::PAGE_SIZE,
            )
        };
        self.control_transfer(
            slot,
            0x80,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DT_DEVICE as u16) << 8,
            0,
            Some(&mut buffer),
            DeviceDescriptor::encoded_size() as u16,
        )?;
        let descriptor = unsafe { read_unaligned(buffer.as_vaddr() as *const DeviceDescriptor) };
        self.log_device_descriptor(slot.usb_device.slot_id(), &descriptor);
        Ok(descriptor)
    }

    fn fetch_configuration_blob(
        &self,
        slot: &SlotRuntime,
    ) -> Result<ContiguousPages, &'static str> {
        let mut header_buffer =
            ContiguousPages::new(1).ok_or("Failed to allocate config header buffer")?;
        unsafe {
            core::ptr::write_bytes(
                header_buffer.as_vaddr() as *mut u8,
                0,
                crate::environment::PAGE_SIZE,
            )
        };
        self.control_transfer(
            slot,
            0x80,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DT_CONFIGURATION as u16) << 8,
            0,
            Some(&mut header_buffer),
            ConfigurationDescriptor::encoded_size() as u16,
        )?;
        let header =
            unsafe { read_unaligned(header_buffer.as_vaddr() as *const ConfigurationDescriptor) };
        self.get_configuration_blob(slot, header.total_length)
    }

    fn get_configuration_blob(
        &self,
        slot: &SlotRuntime,
        total_length: u16,
    ) -> Result<ContiguousPages, &'static str> {
        let minimum = ConfigurationDescriptor::encoded_size() as u16;
        if total_length < minimum {
            println!(
                "[xHCI] Invalid configuration descriptor total_length={}",
                total_length
            );
            return Err("Invalid config descriptor length");
        }
        let pages = (total_length as usize).div_ceil(crate::environment::PAGE_SIZE);
        let mut buffer = ContiguousPages::new(pages).ok_or("Failed to allocate config buffer")?;
        unsafe {
            core::ptr::write_bytes(
                buffer.as_vaddr() as *mut u8,
                0,
                pages * crate::environment::PAGE_SIZE,
            )
        };
        self.control_transfer(
            slot,
            0x80,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DT_CONFIGURATION as u16) << 8,
            0,
            Some(&mut buffer),
            total_length,
        )?;
        self.log_configuration_blob(slot.usb_device.slot_id(), &buffer, total_length);
        Ok(buffer)
    }

    fn get_hub_descriptor(&self, slot: &SlotRuntime) -> Result<HubDescriptor, &'static str> {
        let mut buffer =
            ContiguousPages::new(1).ok_or("Failed to allocate hub descriptor buffer")?;
        unsafe {
            core::ptr::write_bytes(
                buffer.as_vaddr() as *mut u8,
                0,
                crate::environment::PAGE_SIZE,
            )
        };
        self.control_transfer(
            slot,
            0xa0,
            USB_REQ_GET_DESCRIPTOR,
            (USB_DT_HUB as u16) << 8,
            0,
            Some(&mut buffer),
            HubDescriptor::encoded_size() as u16,
        )?;
        let descriptor = unsafe { read_unaligned(buffer.as_vaddr() as *const HubDescriptor) };
        let length = descriptor.length;
        let descriptor_type = descriptor.descriptor_type;
        let num_ports = descriptor.num_ports;
        let characteristics = descriptor.characteristics;
        let power_on_to_power_good = descriptor.power_on_to_power_good;
        let controller_current = descriptor.controller_current;
        println!(
            "[xHCI] Slot {} hub descriptor: len={} type={:#04x} ports={} characteristics={:#06x} pgood={} current={}",
            slot.usb_device.slot_id(),
            length,
            descriptor_type,
            num_ports,
            characteristics,
            power_on_to_power_good,
            controller_current
        );
        if descriptor_type != USB_DT_HUB || length < HubDescriptor::encoded_size() as u8 {
            return Err("Invalid hub descriptor");
        }

        Ok(descriptor)
    }

    fn log_device_descriptor(&self, slot_id: u8, descriptor: &DeviceDescriptor) {
        let vendor_id = descriptor.vendor_id;
        let product_id = descriptor.product_id;
        let device_class = descriptor.device_class;
        let device_subclass = descriptor.device_subclass;
        let device_protocol = descriptor.device_protocol;
        let max_packet_size0 = descriptor.max_packet_size0;
        let num_configurations = descriptor.num_configurations;
        println!(
            "[xHCI] Slot {} device descriptor: vid={:#06x} pid={:#06x} class={:#04x} subclass={:#04x} protocol={:#04x} ep0_mps={} configs={}",
            slot_id,
            vendor_id,
            product_id,
            device_class,
            device_subclass,
            device_protocol,
            max_packet_size0,
            num_configurations
        );
    }

    fn log_configuration_blob(&self, slot_id: u8, blob: &ContiguousPages, total_length: u16) {
        if !XHCI_VERBOSE_TRACE {
            return;
        }

        let total = (total_length as usize).min(blob.len() * crate::environment::PAGE_SIZE);
        let base = blob.as_vaddr();
        let mut offset = 0usize;

        println!(
            "[xHCI] Slot {} configuration descriptor total_length={}",
            slot_id, total_length
        );

        while offset + size_of::<DescriptorHeader>() <= total {
            let header = unsafe { read_unaligned((base + offset) as *const DescriptorHeader) };
            if header.length == 0 || offset + header.length as usize > total {
                println!(
                    "[xHCI] Slot {} descriptor parse stopped: offset={} len={} type={:#04x}",
                    slot_id, offset, header.length, header.descriptor_type
                );
                break;
            }

            match header.descriptor_type {
                USB_DT_CONFIGURATION
                    if header.length as usize >= ConfigurationDescriptor::encoded_size() =>
                {
                    let cfg = unsafe {
                        read_unaligned((base + offset) as *const ConfigurationDescriptor)
                    };
                    let configuration_value = cfg.configuration_value;
                    let num_interfaces = cfg.num_interfaces;
                    let attributes = cfg.attributes;
                    let max_power = cfg.max_power;
                    println!(
                        "[xHCI] Slot {} config: value={} interfaces={} attributes={:#04x} max_power={}",
                        slot_id, configuration_value, num_interfaces, attributes, max_power
                    );
                }
                USB_DT_INTERFACE
                    if header.length as usize >= InterfaceDescriptor::encoded_size() =>
                {
                    let interface =
                        unsafe { read_unaligned((base + offset) as *const InterfaceDescriptor) };
                    let interface_number = interface.interface_number;
                    let alternate_setting = interface.alternate_setting;
                    let num_endpoints = interface.num_endpoints;
                    let interface_class = interface.interface_class;
                    let interface_subclass = interface.interface_subclass;
                    let interface_protocol = interface.interface_protocol;
                    println!(
                        "[xHCI] Slot {} interface: number={} alt={} endpoints={} class={:#04x} subclass={:#04x} protocol={:#04x}",
                        slot_id,
                        interface_number,
                        alternate_setting,
                        num_endpoints,
                        interface_class,
                        interface_subclass,
                        interface_protocol
                    );
                }
                USB_DT_ENDPOINT if header.length as usize >= EndpointDescriptor::encoded_size() => {
                    let endpoint =
                        unsafe { read_unaligned((base + offset) as *const EndpointDescriptor) };
                    let endpoint_address = endpoint.endpoint_address;
                    let attributes = endpoint.attributes;
                    let max_packet_size = endpoint.max_packet_size;
                    let interval = endpoint.interval;
                    println!(
                        "[xHCI] Slot {} endpoint: addr={:#04x} attributes={:#04x} max_packet={} interval={}",
                        slot_id, endpoint_address, attributes, max_packet_size, interval
                    );
                }
                _ => {
                    println!(
                        "[xHCI] Slot {} descriptor: offset={} len={} type={:#04x}",
                        slot_id, offset, header.length, header.descriptor_type
                    );
                }
            }

            offset += header.length as usize;
        }
    }

    fn parse_hub_interface(
        &self,
        blob: &ContiguousPages,
    ) -> Result<HubInterfaceConfig, &'static str> {
        let total = blob.len() * crate::environment::PAGE_SIZE;
        let base = blob.as_vaddr();
        let mut offset = 0usize;
        let mut current_config = 0u8;
        let mut selected: Option<HubInterfaceConfig> = None;

        while offset + size_of::<DescriptorHeader>() <= total {
            let header = unsafe { read_unaligned((base + offset) as *const DescriptorHeader) };
            if header.length == 0 {
                break;
            }

            match header.descriptor_type {
                USB_DT_CONFIGURATION
                    if header.length as usize >= ConfigurationDescriptor::encoded_size() =>
                {
                    let cfg = unsafe {
                        read_unaligned((base + offset) as *const ConfigurationDescriptor)
                    };
                    current_config = cfg.configuration_value;
                }
                USB_DT_INTERFACE
                    if header.length as usize >= InterfaceDescriptor::encoded_size() =>
                {
                    let interface =
                        unsafe { read_unaligned((base + offset) as *const InterfaceDescriptor) };
                    if interface.interface_class == USB_CLASS_HUB {
                        let candidate = HubInterfaceConfig {
                            configuration_value: current_config,
                            interface_number: interface.interface_number,
                            alternate_setting: interface.alternate_setting,
                            protocol: interface.interface_protocol,
                        };
                        if selected
                            .map(|current| candidate.protocol > current.protocol)
                            .unwrap_or(true)
                        {
                            selected = Some(candidate);
                        }
                    }
                }
                _ => {}
            }

            offset += header.length as usize;
        }

        selected.ok_or("No USB hub interface found")
    }

    fn parse_boot_interface(
        &self,
        blob: &ContiguousPages,
    ) -> Result<BootInterfaceConfig, &'static str> {
        let total = blob.len() * crate::environment::PAGE_SIZE;
        let base = blob.as_vaddr();
        let mut offset = 0usize;
        let mut current_config = 0u8;
        let mut current_interface: Option<(u8, HidBootProtocol)> = None;

        while offset + size_of::<DescriptorHeader>() <= total {
            let header = unsafe { read_unaligned((base + offset) as *const DescriptorHeader) };
            if header.length == 0 {
                break;
            }

            match header.descriptor_type {
                USB_DT_CONFIGURATION
                    if header.length as usize >= ConfigurationDescriptor::encoded_size() =>
                {
                    let cfg = unsafe {
                        read_unaligned((base + offset) as *const ConfigurationDescriptor)
                    };
                    current_config = cfg.configuration_value;
                }
                USB_DT_INTERFACE
                    if header.length as usize >= InterfaceDescriptor::encoded_size() =>
                {
                    let interface =
                        unsafe { read_unaligned((base + offset) as *const InterfaceDescriptor) };
                    current_interface = boot_protocol_for_interface(
                        interface.interface_subclass,
                        interface.interface_protocol,
                    )
                    .map(|protocol| (interface.interface_number, protocol));
                }
                USB_DT_ENDPOINT if header.length as usize >= EndpointDescriptor::encoded_size() => {
                    if let Some((interface_number, protocol)) = current_interface {
                        let endpoint =
                            unsafe { read_unaligned((base + offset) as *const EndpointDescriptor) };
                        if endpoint.attributes & 0x3 == USB_ENDPOINT_XFER_INT
                            && (endpoint.endpoint_address & 0x80) != 0
                        {
                            return Ok(BootInterfaceConfig {
                                protocol,
                                configuration_value: current_config,
                                interface_number,
                                endpoint_address: endpoint.endpoint_address,
                                max_packet_size: endpoint.max_packet_size,
                                interval: endpoint.interval,
                            });
                        }
                    }
                }
                _ => {}
            }

            offset += header.length as usize;
        }

        Err("No HID boot interface found")
    }

    fn parse_mass_storage_interface(
        &self,
        blob: &ContiguousPages,
    ) -> Result<MassStorageInterfaceConfig, &'static str> {
        let total = blob.len() * crate::environment::PAGE_SIZE;
        let base = blob.as_vaddr();
        let mut offset = 0usize;
        let mut current_config = 0u8;
        let mut current_interface: Option<u8> = None;
        let mut bulk_in: Option<(u8, u16)> = None;
        let mut bulk_out: Option<(u8, u16)> = None;

        while offset + size_of::<DescriptorHeader>() <= total {
            let header = unsafe { read_unaligned((base + offset) as *const DescriptorHeader) };
            if header.length == 0 {
                break;
            }

            match header.descriptor_type {
                USB_DT_CONFIGURATION
                    if header.length as usize >= ConfigurationDescriptor::encoded_size() =>
                {
                    let cfg = unsafe {
                        read_unaligned((base + offset) as *const ConfigurationDescriptor)
                    };
                    current_config = cfg.configuration_value;
                }
                USB_DT_INTERFACE
                    if header.length as usize >= InterfaceDescriptor::encoded_size() =>
                {
                    let interface =
                        unsafe { read_unaligned((base + offset) as *const InterfaceDescriptor) };
                    let is_msc = interface.interface_class == USB_CLASS_MASS_STORAGE
                        && interface.interface_subclass == USB_MSC_SUBCLASS_SCSI
                        && interface.interface_protocol == USB_MSC_PROTOCOL_BULK_ONLY;
                    current_interface = is_msc.then_some(interface.interface_number);
                    bulk_in = None;
                    bulk_out = None;
                }
                USB_DT_ENDPOINT if header.length as usize >= EndpointDescriptor::encoded_size() => {
                    if let Some(interface_number) = current_interface {
                        let endpoint =
                            unsafe { read_unaligned((base + offset) as *const EndpointDescriptor) };
                        if endpoint.attributes & 0x3 == USB_ENDPOINT_XFER_BULK {
                            let endpoint_address = endpoint.endpoint_address;
                            let max_packet_size = endpoint.max_packet_size;
                            if endpoint_address & 0x80 != 0 {
                                bulk_in = Some((endpoint_address, max_packet_size));
                            } else {
                                bulk_out = Some((endpoint_address, max_packet_size));
                            }

                            if let (
                                Some((bulk_in_endpoint, bulk_in_max_packet_size)),
                                Some((bulk_out_endpoint, bulk_out_max_packet_size)),
                            ) = (bulk_in, bulk_out)
                            {
                                return Ok(MassStorageInterfaceConfig {
                                    configuration_value: current_config,
                                    interface_number,
                                    bulk_in_endpoint,
                                    bulk_in_max_packet_size,
                                    bulk_out_endpoint,
                                    bulk_out_max_packet_size,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }

            offset += header.length as usize;
        }

        Err("No USB mass storage bulk-only interface found")
    }

    fn interrupt_dci(endpoint_address: u8) -> u8 {
        Self::endpoint_dci(endpoint_address)
    }

    fn endpoint_dci(endpoint_address: u8) -> u8 {
        let ep_num = endpoint_address & 0x0f;
        if endpoint_address & 0x80 != 0 {
            ep_num * 2 + 1
        } else {
            ep_num * 2
        }
    }

    fn xhci_interval(speed: UsbSpeed, b_interval: u8) -> u8 {
        match speed {
            UsbSpeed::High | UsbSpeed::Super | UsbSpeed::SuperPlus => {
                b_interval.saturating_sub(1).clamp(0, 15)
            }
            UsbSpeed::Full | UsbSpeed::Low => {
                if b_interval == 0 {
                    0
                } else {
                    let ms = b_interval as u32;
                    let frames_125us = ms * 8;
                    (32u32 - frames_125us.leading_zeros()).clamp(0, 15) as u8
                }
            }
        }
    }

    pub fn enumerate_ports(&self) -> Result<usize, &'static str> {
        let mut discovered = 0usize;
        for port_id in 1..=self.max_ports {
            let portsc = self.read_portsc(port_id);
            let status = PortStatus::from_portsc(portsc);
            self.log_port_status(port_id, portsc);
            self.clear_port_change_bits(port_id, portsc);
            if !status.connected {
                continue;
            }
            if self
                .devices
                .lock()
                .iter()
                .any(|device| device.port_id() == port_id)
            {
                continue;
            }
            let status = self.reset_port(port_id)?;
            let speed = status.speed();
            println!("[xHCI] Port {} reset complete, speed {:?}", port_id, speed);
            let completion = self.send_command(Trb::enable_slot_command())?;
            let slot_id = completion.slot_id();
            if slot_id == 0 {
                return Err("Enable Slot returned slot 0");
            }
            let slot_runtime = self.address_device(slot_id, port_id, speed, 0, 0, None, 0)?;
            self.devices.lock().push(slot_runtime.usb_device);
            self.slot_runtime.lock().push(slot_runtime);
            self.configure_known_classes(slot_id);
            discovered += 1;
        }
        Ok(discovered)
    }

    pub fn devices(&self) -> Vec<UsbDevice> {
        self.devices.lock().clone()
    }

    fn bulk_transfer(
        &self,
        slot_id: u8,
        endpoint_address: u8,
        buffer: &mut ContiguousPages,
        length: usize,
    ) -> Result<usize, &'static str> {
        if length > USB_BULK_MAX_TRANSFER {
            return Err("USB bulk transfer too large");
        }

        let dci = Self::endpoint_dci(endpoint_address);
        let direction_in;
        {
            let slots = self.slot_runtime.lock();
            let slot = slots
                .iter()
                .find(|slot| slot.usb_device.slot_id() == slot_id)
                .ok_or("Unknown slot for bulk transfer")?;
            let storage = slot
                .storage
                .as_ref()
                .ok_or("Mass storage endpoints not configured")?;
            let _context_paddr = storage.input_context.as_paddr();

            let ring = if storage.bulk_in.endpoint_address == endpoint_address {
                if storage.bulk_in.dci != dci {
                    return Err("Bulk IN endpoint DCI mismatch");
                }
                let _max_packet_size = storage.bulk_in.max_packet_size;
                &storage.bulk_in.ring
            } else if storage.bulk_out.endpoint_address == endpoint_address {
                if storage.bulk_out.dci != dci {
                    return Err("Bulk OUT endpoint DCI mismatch");
                }
                let _max_packet_size = storage.bulk_out.max_packet_size;
                &storage.bulk_out.ring
            } else {
                return Err("Bulk endpoint not configured for slot");
            };

            direction_in = storage.bulk_in.endpoint_address == endpoint_address;
            let flags = if direction_in {
                IommuMapFlags::WRITE | IommuMapFlags::COHERENT
            } else {
                IommuMapFlags::READ | IommuMapFlags::COHERENT
            };
            if direction_in {
                sync_pages_before_device_write(buffer);
            } else {
                sync_pages_for_device(buffer);
            }
            let buffer_dma_addr = self.dma_map_pages(buffer, flags)?;
            let trb = if direction_in {
                Trb::normal_transfer_in(buffer_dma_addr as u64, length as u32)
            } else {
                Trb::normal_transfer(buffer_dma_addr as u64, length as u32)
            };
            let deferred_trb = ring.enqueue_deferred_cycle(trb)?;
            if XHCI_VERBOSE_TRACE {
                println!(
                    "[xHCI] Bulk submit: slot={} ep_addr={:#x} dci={} dir={} len={} buffer_dma={:#x} ring_dma={:#x} trb_index={}",
                    slot_id,
                    endpoint_address,
                    dci,
                    if direction_in { "in" } else { "out" },
                    length,
                    buffer_dma_addr,
                    ring.dma_address(),
                    deferred_trb.index
                );
            }
            ring.publish_deferred_cycle(deferred_trb)?;
        }

        self.ring_endpoint_doorbell(slot_id, dci);
        let event = self.wait_for_transfer_event(slot_id, dci)?;
        if direction_in {
            sync_pages_after_device_write(buffer);
        }
        Ok(length.saturating_sub(event.transfer_length() as usize))
    }

    fn attach_mass_storage_devices(self: &Arc<Self>) {
        let slot_ids: Vec<u8> = self
            .slot_runtime
            .lock()
            .iter()
            .map(|slot| slot.usb_device.slot_id())
            .collect();
        for slot_id in slot_ids {
            match self.configure_mass_storage_slot(slot_id) {
                Ok(true) => println!("[xHCI] Slot {} mass storage configured", slot_id),
                Ok(false) => {}
                Err(error) => {
                    println!(
                        "[xHCI] Slot {} mass storage setup failed: {}",
                        slot_id, error
                    )
                }
            }
        }
    }

    fn slot_runtime_mut(&self, slot_id: u8) -> Option<spin::MutexGuard<'_, Vec<SlotRuntime>>> {
        let guard = self.slot_runtime.lock();
        if guard
            .iter()
            .any(|slot| slot.usb_device.slot_id() == slot_id)
        {
            Some(guard)
        } else {
            None
        }
    }

    pub fn attach_boot_keyboard(&self, slot_id: u8) -> Result<(), &'static str> {
        let mut slots = self.slot_runtime.lock();
        let slot = slots
            .iter_mut()
            .find(|slot| slot.usb_device.slot_id() == slot_id)
            .ok_or("Unknown slot for keyboard attach")?;

        let keyboard = HidKeyboardDevice::new();
        let event_device = keyboard.event_device();
        let name = event_device.get_name().to_string();
        let as_device: Arc<dyn Device> = event_device.clone();
        DeviceManager::get_manager().register_device_with_name(name, as_device);
        slot.hid = Some(HidDeviceState::Keyboard(keyboard));
        Ok(())
    }

    pub fn attach_boot_mouse(&self, slot_id: u8) -> Result<(), &'static str> {
        let mut slots = self.slot_runtime.lock();
        let slot = slots
            .iter_mut()
            .find(|slot| slot.usb_device.slot_id() == slot_id)
            .ok_or("Unknown slot for mouse attach")?;

        let mouse = HidMouseDevice::new();
        let event_device = mouse.event_device();
        let name = event_device.get_name().to_string();
        let as_device: Arc<dyn Device> = event_device.clone();
        DeviceManager::get_manager().register_device_with_name(name, as_device);
        slot.hid = Some(HidDeviceState::Mouse(mouse));
        Ok(())
    }

    fn handle_transfer_event(&self, event: Trb) -> bool {
        let slot_id = event.slot_id();
        let endpoint_id = event.endpoint_id();
        let mut slots = self.slot_runtime.lock();
        let Some(slot) = slots
            .iter_mut()
            .find(|slot| slot.usb_device.slot_id() == slot_id)
        else {
            return false;
        };

        if slot.interrupt_dci != Some(endpoint_id) {
            return false;
        }

        let Some(buffer) = slot.interrupt_buffer.as_ref() else {
            return true;
        };

        match slot.hid.as_mut() {
            Some(HidDeviceState::Keyboard(keyboard)) => {
                if KeyboardBootReport::encoded_size()
                    <= buffer.len() * crate::environment::PAGE_SIZE
                {
                    let report =
                        unsafe { read_volatile(buffer.as_vaddr() as *const KeyboardBootReport) };
                    keyboard.handle_report(report);
                }
            }
            Some(HidDeviceState::Mouse(mouse)) => {
                if MouseBootReport::encoded_size() <= buffer.len() * crate::environment::PAGE_SIZE {
                    let report =
                        unsafe { read_volatile(buffer.as_vaddr() as *const MouseBootReport) };
                    mouse.handle_report(report);
                }
            }
            None => {}
        }

        drop(slots);
        if let Err(error) = self.submit_interrupt_in_transfer(slot_id) {
            println!(
                "[xHCI] Failed to resubmit interrupt transfer for slot {}: {}",
                slot_id, error
            );
        }
        true
    }

    fn process_interrupt_events(&self) {
        while let Some(event) = self.poll_event() {
            match event.trb_type() {
                value if value == TrbType::TransferEvent as u8 => {
                    if !self.handle_transfer_event(event) {
                        self.queue_pending_event(event);
                    }
                }
                value if value == TrbType::PortStatusChangeEvent as u8 => {
                    self.handle_port_change_detected();
                }
                value if value == TrbType::CommandCompletionEvent as u8 => {
                    self.queue_pending_event(event);
                }
                _ => {}
            }
        }
    }

    fn handle_port_change_detected(&self) {
        println!("[xHCI] Port change detected");
        match self.enumerate_ports() {
            Ok(count) => {
                if count != 0 {
                    println!("[xHCI] Port change enumerated {} device(s)", count);
                }
            }
            Err(error) => println!("[xHCI] Port change enumeration failed: {}", error),
        }
    }

    /// Get the number of supported device slots
    pub fn max_slots(&self) -> u8 {
        self.max_slots
    }

    /// Get the number of ports
    pub fn max_ports(&self) -> u8 {
        self.max_ports
    }

    /// Get the MMIO base address
    pub fn mmio_base(&self) -> usize {
        self.mmio_base
    }
}

static USB_BLOCK_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn next_usb_block_device_name() -> alloc::string::String {
    alloc::format!("usbblk{}", USB_BLOCK_COUNTER.fetch_add(1, Ordering::SeqCst))
}

struct UsbMassStorageBlockDevice {
    controller: Arc<XhciController>,
    slot_id: u8,
    bulk_in_endpoint: u8,
    bulk_out_endpoint: u8,
    sector_size: Mutex<usize>,
    sector_count: Mutex<usize>,
    request_queue: Mutex<VecDeque<Box<BlockIORequest>>>,
    command_lock: Mutex<()>,
    next_tag: AtomicUsize,
}

impl UsbMassStorageBlockDevice {
    fn new(
        controller: Arc<XhciController>,
        slot_id: u8,
        bulk_in_endpoint: u8,
        bulk_out_endpoint: u8,
    ) -> Self {
        Self {
            controller,
            slot_id,
            bulk_in_endpoint,
            bulk_out_endpoint,
            sector_size: Mutex::new(512),
            sector_count: Mutex::new(0),
            request_queue: Mutex::new(VecDeque::new()),
            command_lock: Mutex::new(()),
            next_tag: AtomicUsize::new(1),
        }
    }

    fn initialize(&self) -> Result<(), &'static str> {
        let mut inquiry = ContiguousPages::new(1).ok_or("Failed to allocate INQUIRY buffer")?;
        self.scsi_command(&[0x12, 0, 0, 0, 36, 0], Some((&mut inquiry, 36, true)))?;

        self.scsi_command(&[0x00, 0, 0, 0, 0, 0], None)?;

        let mut capacity = ContiguousPages::new(1).ok_or("Failed to allocate capacity buffer")?;
        self.scsi_command(
            &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            Some((&mut capacity, 8, true)),
        )?;

        let data = unsafe { core::slice::from_raw_parts(capacity.as_vaddr() as *const u8, 8) };
        let last_lba = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let block_len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        if block_len == 0 {
            return Err("USB storage reported zero block size");
        }

        *self.sector_size.lock() = block_len;
        *self.sector_count.lock() = last_lba.saturating_add(1);
        println!(
            "[usb-storage] capacity sectors={} sector_size={}",
            last_lba.saturating_add(1),
            block_len
        );
        Ok(())
    }

    fn scsi_command(
        &self,
        command: &[u8],
        data_stage: Option<(&mut ContiguousPages, usize, bool)>,
    ) -> Result<(), &'static str> {
        if command.len() > 16 {
            return Err("SCSI command too large for BOT CBW");
        }

        let _command_guard = self.command_lock.lock();

        let (data_len, direction_in) = data_stage
            .as_ref()
            .map(|(_, len, direction_in)| (*len, *direction_in))
            .unwrap_or((0, false));
        let tag = self.next_tag.fetch_add(1, Ordering::SeqCst) as u32;

        let mut cbw = ContiguousPages::new(1).ok_or("Failed to allocate USB BOT CBW")?;
        unsafe {
            core::ptr::write_bytes(cbw.as_vaddr() as *mut u8, 0, crate::environment::PAGE_SIZE);
            let bytes =
                core::slice::from_raw_parts_mut(cbw.as_vaddr() as *mut u8, USB_STORAGE_CBW_LEN);
            bytes[0..4].copy_from_slice(&USB_STORAGE_CBW_SIGNATURE.to_le_bytes());
            bytes[4..8].copy_from_slice(&tag.to_le_bytes());
            bytes[8..12].copy_from_slice(&(data_len as u32).to_le_bytes());
            bytes[12] = if direction_in { 0x80 } else { 0x00 };
            bytes[13] = 0;
            bytes[14] = command.len() as u8;
            bytes[15..15 + command.len()].copy_from_slice(command);
        }

        let transferred = self.controller.bulk_transfer(
            self.slot_id,
            self.bulk_out_endpoint,
            &mut cbw,
            USB_STORAGE_CBW_LEN,
        )?;
        if transferred != USB_STORAGE_CBW_LEN {
            return Err("Short USB BOT CBW transfer");
        }

        if let Some((buffer, len, is_in)) = data_stage {
            let endpoint = if is_in {
                self.bulk_in_endpoint
            } else {
                self.bulk_out_endpoint
            };
            let transferred = self
                .controller
                .bulk_transfer(self.slot_id, endpoint, buffer, len)?;
            if transferred != len {
                return Err("Short USB BOT data transfer");
            }
        }

        let mut csw = ContiguousPages::new(1).ok_or("Failed to allocate USB BOT CSW")?;
        unsafe {
            core::ptr::write_bytes(csw.as_vaddr() as *mut u8, 0, crate::environment::PAGE_SIZE);
        }
        let transferred = self.controller.bulk_transfer(
            self.slot_id,
            self.bulk_in_endpoint,
            &mut csw,
            USB_STORAGE_CSW_LEN,
        )?;
        if transferred != USB_STORAGE_CSW_LEN {
            return Err("Short USB BOT CSW transfer");
        }

        let csw_bytes = unsafe {
            core::slice::from_raw_parts(csw.as_vaddr() as *const u8, USB_STORAGE_CSW_LEN)
        };
        let signature =
            u32::from_le_bytes([csw_bytes[0], csw_bytes[1], csw_bytes[2], csw_bytes[3]]);
        let returned_tag =
            u32::from_le_bytes([csw_bytes[4], csw_bytes[5], csw_bytes[6], csw_bytes[7]]);
        let status = csw_bytes[12];
        if signature != USB_STORAGE_CSW_SIGNATURE {
            return Err("Invalid USB BOT CSW signature");
        }
        if returned_tag != tag {
            return Err("USB BOT CSW tag mismatch");
        }
        if status != 0 {
            return Err("USB storage SCSI command failed");
        }

        Ok(())
    }

    fn read_sectors(&self, request: &mut BlockIORequest) -> Result<(), &'static str> {
        let sector_size = *self.sector_size.lock();
        let sector_count = *self.sector_count.lock();
        if sector_size == 0 {
            return Err("USB storage sector size not initialized");
        }
        if request.sector_count == 0 {
            request.buffer.clear();
            return Ok(());
        }
        if request.sector >= sector_count
            || request.sector_count > sector_count.saturating_sub(request.sector)
        {
            return Err("USB storage read out of range");
        }

        let total_len = request
            .sector_count
            .checked_mul(sector_size)
            .ok_or("USB storage read length overflow")?;
        if request.buffer.len() != total_len {
            request.buffer.resize(total_len, 0);
        }

        let max_blocks = (USB_BULK_MAX_TRANSFER / sector_size)
            .max(1)
            .min(u16::MAX as usize);
        let mut done_blocks = 0usize;
        while done_blocks < request.sector_count {
            let blocks = (request.sector_count - done_blocks).min(max_blocks);
            let bytes = blocks * sector_size;
            let pages = bytes.div_ceil(crate::environment::PAGE_SIZE);
            let mut buffer =
                ContiguousPages::new(pages).ok_or("Failed to allocate USB read buffer")?;
            unsafe {
                core::ptr::write_bytes(
                    buffer.as_vaddr() as *mut u8,
                    0,
                    pages * crate::environment::PAGE_SIZE,
                );
            }

            let lba = request.sector + done_blocks;
            let command = scsi_rw10_command(SCSI_READ_10, lba, blocks)?;
            self.scsi_command(&command, Some((&mut buffer, bytes, true)))?;

            let dst_offset = done_blocks * sector_size;
            unsafe {
                let src = core::slice::from_raw_parts(buffer.as_vaddr() as *const u8, bytes);
                request.buffer[dst_offset..dst_offset + bytes].copy_from_slice(src);
            }
            done_blocks += blocks;
        }

        Ok(())
    }

    fn write_sectors(&self, request: &mut BlockIORequest) -> Result<(), &'static str> {
        let sector_size = *self.sector_size.lock();
        let sector_count = *self.sector_count.lock();
        if sector_size == 0 {
            return Err("USB storage sector size not initialized");
        }
        if request.sector_count == 0 {
            return Ok(());
        }
        if request.sector >= sector_count
            || request.sector_count > sector_count.saturating_sub(request.sector)
        {
            return Err("USB storage write out of range");
        }

        let total_len = request
            .sector_count
            .checked_mul(sector_size)
            .ok_or("USB storage write length overflow")?;
        if request.buffer.len() < total_len {
            return Err("USB storage write buffer too small");
        }

        let max_blocks = (USB_BULK_MAX_TRANSFER / sector_size)
            .max(1)
            .min(u16::MAX as usize);
        let mut done_blocks = 0usize;
        while done_blocks < request.sector_count {
            let blocks = (request.sector_count - done_blocks).min(max_blocks);
            let bytes = blocks * sector_size;
            let pages = bytes.div_ceil(crate::environment::PAGE_SIZE);
            let mut buffer =
                ContiguousPages::new(pages).ok_or("Failed to allocate USB write buffer")?;
            let src_offset = done_blocks * sector_size;
            unsafe {
                core::ptr::write_bytes(
                    buffer.as_vaddr() as *mut u8,
                    0,
                    pages * crate::environment::PAGE_SIZE,
                );
                let dst = core::slice::from_raw_parts_mut(buffer.as_vaddr() as *mut u8, bytes);
                dst.copy_from_slice(&request.buffer[src_offset..src_offset + bytes]);
            }

            let lba = request.sector + done_blocks;
            let command = scsi_rw10_command(SCSI_WRITE_10, lba, blocks)?;
            self.scsi_command(&command, Some((&mut buffer, bytes, false)))?;

            done_blocks += blocks;
        }

        Ok(())
    }

    fn process_request(&self, request: &mut BlockIORequest) -> Result<(), &'static str> {
        match request.request_type {
            BlockIORequestType::Read => self.read_sectors(request),
            BlockIORequestType::Write => self.write_sectors(request),
        }
    }
}

impl Device for UsbMassStorageBlockDevice {
    fn device_type(&self) -> crate::device::DeviceType {
        crate::device::DeviceType::Block
    }

    fn name(&self) -> &'static str {
        "usb-storage"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_block_device(&self) -> Option<&dyn BlockDevice> {
        Some(self)
    }

    fn into_block_device(self: Arc<Self>) -> Option<Arc<dyn BlockDevice>> {
        Some(self)
    }
}

impl BlockDevice for UsbMassStorageBlockDevice {
    fn get_disk_name(&self) -> &'static str {
        "usb-storage"
    }

    fn get_disk_size(&self) -> usize {
        *self.sector_count.lock() * *self.sector_size.lock()
    }

    fn get_sector_size(&self) -> usize {
        *self.sector_size.lock()
    }

    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        self.request_queue.lock().push_back(request);
    }

    fn process_requests(&self) -> Vec<BlockIOResult> {
        let requests = {
            let mut queue = self.request_queue.lock();
            let mut requests = Vec::new();
            while let Some(request) = queue.pop_front() {
                requests.push(request);
            }
            requests
        };

        let mut results = Vec::new();
        for mut request in requests {
            let result = self.process_request(&mut request);
            results.push(BlockIOResult { request, result });
        }
        results
    }
}

impl ControlOps for UsbMassStorageBlockDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for UsbMassStorageBlockDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported by USB storage")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for UsbMassStorageBlockDevice {
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

/// Decodes a memory BAR into a normalized MMIO description.
pub fn decode_mmio_bar(low: u32, high: Option<u32>) -> Option<PciBar> {
    // Bit 0 indicates I/O space (1) or memory space (0)
    if (low & 0x1) != 0 {
        return None; // I/O BAR, not supported
    }

    let bar_type = (low >> 1) & 0x3;
    let is_64bit = bar_type == 0x2;
    let prefetchable = (low & 0x8) != 0;

    let base = if is_64bit {
        let high = high? as u64;
        ((high << 32) | ((low & !0xf) as u64)) as usize
    } else {
        (low & !0xf) as usize
    };

    Some(PciBar {
        base,
        size: 0, // Size needs to be determined separately
        is_memory: true,
        is_64bit,
        prefetchable,
    })
}

/// Determine BAR size by writing 0xFFFFFFFF and reading back
fn determine_bar_size(
    config: &PciConfig,
    addr: &crate::device::pci::PciAddress,
    bar_offset: usize,
) -> usize {
    let original_low = config.read_u32(addr, bar_offset);
    if (original_low & 0x1) != 0 {
        return 0;
    }

    let is_64bit = (original_low & 0x6) == 0x4;
    let probe_low = (original_low & 0xF) | 0xFFFF_FFF0;

    if is_64bit {
        let original_high = config.read_u32(addr, bar_offset + 4);

        config.write_u32(addr, bar_offset, probe_low);
        config.write_u32(addr, bar_offset + 4, 0xFFFF_FFFF);

        let size_low = config.read_u32(addr, bar_offset);
        let size_high = config.read_u32(addr, bar_offset + 4);

        config.write_u32(addr, bar_offset, original_low);
        config.write_u32(addr, bar_offset + 4, original_high);

        let mask = ((size_high as u64) << 32) | (u64::from(size_low) & 0xFFFF_FFF0);
        if mask == 0 {
            return 0;
        }

        (!mask).wrapping_add(1) as usize
    } else {
        config.write_u32(addr, bar_offset, probe_low);

        let size_low = config.read_u32(addr, bar_offset);

        config.write_u32(addr, bar_offset, original_low);

        let mask = size_low & 0xFFFF_FFF0;
        if mask == 0 {
            return 0;
        }

        (!mask).wrapping_add(1) as usize
    }
}

impl UsbHostController for XhciController {
    fn poll_events(&self) {
        let status = self.operational.read_usbsts();
        let pending = status & (USBSTS_EVENT_INTERRUPT | USBSTS_PORT_CHANGE_DETECT);
        if pending != 0 {
            let port_change = (pending & USBSTS_PORT_CHANGE_DETECT) != 0;
            self.operational.write_usbsts(pending);
            self.process_interrupt_events();
            if port_change {
                self.handle_port_change_detected();
            }
        }
    }
}

/// xHCI driver instance container
static NEXT_USB_HOST_ID: AtomicUsize = AtomicUsize::new(1);

struct XhciPollHandler;
impl TimerHandler for XhciPollHandler {
    fn on_timer_expired(self: Arc<Self>, _context: usize) {
        DeviceManager::get_manager().for_each_usb_host(|ctrl| {
            ctrl.poll_events();
        });
        let handler: Arc<dyn TimerHandler> = self.clone();
        let expires = crate::timer::get_tick() + crate::timer::ms_to_ticks(500);
        crate::timer::add_timer(expires, &handler, 0);
    }
}

impl InterruptCapableDevice for XhciController {
    fn handle_interrupt(&self) -> crate::interrupt::InterruptResult<()> {
        let _ = self.claim_interrupt()?;
        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        *self.interrupt_id.lock()
    }

    fn claim_interrupt(&self) -> crate::interrupt::InterruptResult<InterruptClaim> {
        let status = self.operational.read_usbsts();
        let pending = status & (USBSTS_EVENT_INTERRUPT | USBSTS_PORT_CHANGE_DETECT);
        if pending == 0 {
            return Ok(InterruptClaim::NotMine);
        }

        let port_change = (pending & USBSTS_PORT_CHANGE_DETECT) != 0;
        self.operational.write_usbsts(pending);

        unsafe {
            write_volatile(
                (self.regs.runtime_base + registers::runtime::IR0_IMAN) as *mut u32,
                (1 << 1) | 1, // keep IE, clear IP
            );
        }

        self.process_interrupt_events();
        if port_change {
            self.handle_port_change_detected();
        }
        Ok(InterruptClaim::Handled)
    }
}

fn initialize_xhci_controller(
    mmio_vaddr: usize,
    dma_context: DmaContext,
) -> Result<Arc<XhciController>, &'static str> {
    println!("[xHCI] Binding platform xHCI at {:#x}", mmio_vaddr);

    let controller = Arc::new(XhciController::new_with_dma_context(
        mmio_vaddr,
        dma_context,
    )?);

    controller.init()?;
    controller.start()?;

    match controller.enumerate_ports() {
        Ok(count) => println!("[xHCI] Enumerated {} device(s)", count),
        Err(error) => println!("[xHCI] Enumeration deferred: {}", error),
    }
    controller.attach_mass_storage_devices();

    Ok(controller)
}

fn register_xhci_host(controller: Arc<XhciController>) {
    let host_id = NEXT_USB_HOST_ID.fetch_add(1, Ordering::SeqCst) as u32;
    let host: Arc<dyn UsbHostController> = controller.clone();
    DeviceManager::get_manager().register_usb_host(host_id, host);

    let poll_handler: Arc<dyn TimerHandler> = Arc::new(XhciPollHandler);
    add_timer(get_tick() + ms_to_ticks(1000), &poll_handler, 0);

    println!("[xHCI] Platform controller registered successfully");
}

/// Bind a platform MMIO xHCI controller.
///
/// # Arguments
///
/// * `mmio_vaddr` - Kernel virtual address of the xHCI MMIO register block.
/// * `interrupt` - Optional platform interrupt ID for the controller.
/// * `dma_context` - DMA mapping context for xHCI-owned rings and contexts.
///
/// # Returns
///
/// `Ok(())` when the controller is initialized and registered.
pub fn bind_xhci_mmio(
    mmio_vaddr: usize,
    interrupt: Option<InterruptId>,
    dma_context: DmaContext,
) -> Result<(), &'static str> {
    let controller = initialize_xhci_controller(mmio_vaddr, dma_context)?;

    if let Some(interrupt_id) = interrupt {
        InterruptManager::global()
            .register_interrupt_device(interrupt_id, controller.clone())
            .map_err(|_| "Failed to register xHCI interrupt device")?;
        InterruptManager::global()
            .enable_external_interrupt(interrupt_id, crate::arch::get_cpu().get_cpuid() as u32)
            .map_err(|_| "Failed to enable xHCI interrupt")?;
        controller.enable_interrupts(interrupt_id)?;
        println!("[xHCI] Registered IRQ {}", interrupt_id);
    } else {
        println!("[xHCI] No interrupt provided for platform controller");
    }

    register_xhci_host(controller);
    Ok(())
}

fn probe_xhci(device: &PciDeviceInfo) -> Result<(), &'static str> {
    println!(
        "[xHCI] Probing device: {:04x}:{:04x}",
        device.vendor_id(),
        device.device_id()
    );

    let config = PciConfig::new(device.ecam_vaddr());
    let addr = device.address();

    // Enable bus mastering
    let command = config.read_u16(&addr, config::offset::COMMAND);
    config.write_u16(
        &addr,
        config::offset::COMMAND,
        command | config::command::BUS_MASTER | config::command::INTERRUPT_DISABLE,
    );
    println!("[xHCI] Bus mastering enabled");

    // Enable memory access
    let command = config.read_u16(&addr, config::offset::COMMAND);
    config.write_u16(
        &addr,
        config::offset::COMMAND,
        command | config::command::MEMORY_SPACE | config::command::INTERRUPT_DISABLE,
    );
    println!("[xHCI] Memory access enabled");

    // Read BAR0 (and BAR1 if 64-bit)
    let bar0_raw = config.read_u32(&addr, config::offset::BAR0);
    let bar = decode_mmio_bar(bar0_raw, None);

    let bar = match bar {
        Some(b) => b,
        None => {
            // Try 64-bit BAR
            let bar1_raw = config.read_u32(&addr, config::offset::BAR0 + 4);
            decode_mmio_bar(bar0_raw, Some(bar1_raw)).ok_or("Failed to decode xHCI BAR0")?
        }
    };

    println!("[xHCI] BAR0: base={:#x}, 64-bit={}", bar.base, bar.is_64bit);

    // Determine BAR size
    let bar_size = determine_bar_size(&config, &addr, config::offset::BAR0);
    println!("[xHCI] BAR0 size: {:#x}", bar_size);

    // Map MMIO region
    let mmio_size = if bar_size > 0 { bar_size } else { 0x10000 }; // Default 64KB
    let mmio_vaddr =
        vm::ioremap(bar.base, mmio_size).map_err(|_| "Failed to map xHCI MMIO region")?;

    println!("[xHCI] MMIO mapped at {:#x}", mmio_vaddr);

    let routed_irq = device.routed_irq();
    let interrupt_line = routed_irq.unwrap_or_else(|| device.interrupt_line() as InterruptId);
    let interrupt_pin = device.interrupt_pin();
    println!(
        "[xHCI] IRQ routing: config_line={} pin={} routed_irq={:?}",
        device.interrupt_line(),
        interrupt_pin,
        routed_irq
    );
    let interrupt_id = if interrupt_line != 0 && interrupt_line != 0xff && interrupt_pin != 0 {
        println!(
            "[xHCI] Registered IRQ {} (pin {})",
            interrupt_line, interrupt_pin
        );
        Some(interrupt_line)
    } else {
        println!("[xHCI] No usable legacy IRQ routing for controller");
        None
    };

    let controller = initialize_xhci_controller(mmio_vaddr, DmaContext::direct())?;

    if let Some(interrupt_id) = interrupt_id {
        controller.enable_interrupts(interrupt_id)?;
        let source = PciIntxInterruptSource::new(device, controller.clone())
            .ok_or("Failed to create xHCI INTx source")?;
        let source: Arc<dyn crate::interrupt::MaskableInterruptSource> = Arc::new(source);
        InterruptManager::global()
            .register_and_enable_interrupt_source(source, crate::arch::get_cpu().get_cpuid() as u32)
            .map_err(|_| "Failed to register xHCI interrupt device")?;
        println!("[xHCI] Registered IRQ {}", interrupt_id);
    } else {
        println!("[xHCI] No usable legacy IRQ routing for controller");
    }

    register_xhci_host(controller);
    Ok(())
}

/// Remove function for xHCI PCI devices
fn remove_xhci(_device: &PciDeviceInfo) -> Result<(), &'static str> {
    // TODO: Implement proper cleanup
    Ok(())
}

/// Register the xHCI driver with the device manager
fn register_driver() {
    // Match xHCI class code (0x0C0330)
    let id_table = alloc::vec![PciDeviceId::from_class(XHCI_CLASS_CODE, 0xFFFFFF),];

    let driver = PciDeviceDriver::new("xhci", id_table, probe_xhci, remove_xhci);

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::pci::PciAddress;

    fn sample_xhci_device() -> PciDeviceInfo {
        PciDeviceInfo::new(
            PciAddress::new(0, 0, 5, 0),
            0,
            0x8086,
            0x1e31,
            XHCI_CLASS_CODE,
            0x01,
            0,
            0,
            11,
            1,
            None,
            "xhci-controller",
            1,
        )
    }

    #[test_case]
    fn test_xhci_pci_class_match() {
        let device = sample_xhci_device();
        let id = PciDeviceId::from_class(XHCI_CLASS_CODE, 0xFFFFFF);
        assert!(id.matches(&device));
    }

    #[test_case]
    fn test_decode_32bit_mmio_bar() {
        let bar = decode_mmio_bar(0xfedc_0000, None).unwrap();
        assert_eq!(bar.base, 0xfedc_0000usize);
        assert!(bar.is_memory);
        assert!(!bar.is_64bit);
    }

    #[test_case]
    fn test_decode_64bit_mmio_bar() {
        let bar = decode_mmio_bar(0x1234_0004, Some(0x0000_0001)).unwrap();
        assert_eq!(bar.base, 0x0000_0001_1234_0000usize);
        assert!(bar.is_64bit);
    }

    #[test_case]
    fn test_reject_io_bar() {
        assert!(decode_mmio_bar(0x0000_1001, None).is_none());
    }

    #[test_case]
    fn test_xhci_capabilities_layout() {
        // Verify capability struct size is reasonable
        assert!(size_of::<XhciCapabilities>() <= 64);
    }

    #[test_case]
    fn test_pci_bar_struct_size() {
        // Verify PciBar size
        assert!(size_of::<PciBar>() <= 32);
    }

    #[test_case]
    fn test_port_status_decodes_rxdetect_unconnected_port() {
        let status = PortStatus::from_portsc(0x2a0);

        assert!(!status.connected);
        assert!(!status.enabled);
        assert_eq!(status.link_state, 5);
        assert!(status.powered);
        assert_eq!(status.speed_raw, 0);
        assert_eq!(status.change_bits, 0);
    }

    #[test_case]
    fn test_port_status_extracts_change_bits() {
        let portsc = PORTSC_CCS | PORTSC_CSC | PORTSC_PLC | PORTSC_CEC;
        let status = PortStatus::from_portsc(portsc);

        assert!(status.connected);
        assert_eq!(status.change_bits, PORTSC_CSC | PORTSC_PLC | PORTSC_CEC);
    }

    #[test_case]
    fn test_portsc_write_preserve_bits_does_not_write_ped_or_changes() {
        let portsc = PORTSC_PP | PORTSC_PED | PORTSC_CSC | PORTSC_PRC | PORTSC_CEC;

        assert_eq!(
            XhciController::portsc_write_preserve_bits(portsc),
            PORTSC_PP
        );
    }

    #[test_case]
    fn test_scsi_read10_command_encoding() {
        let command = scsi_rw10_command(SCSI_READ_10, 0x0102_0304, 0x0506).unwrap();

        assert_eq!(command, [0x28, 0, 0x01, 0x02, 0x03, 0x04, 0, 0x05, 0x06, 0]);
    }

    #[test_case]
    fn test_scsi_write10_command_encoding() {
        let command = scsi_rw10_command(SCSI_WRITE_10, 0x1020_3040, 0x1122).unwrap();

        assert_eq!(command, [0x2a, 0, 0x10, 0x20, 0x30, 0x40, 0, 0x11, 0x22, 0]);
    }

    #[test_case]
    fn test_scsi_rw10_command_rejects_zero_blocks() {
        assert!(scsi_rw10_command(SCSI_READ_10, 0, 0).is_err());
    }
}
