#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::asm;

use scarlet::sync::Mutex;

use scarlet::arch::mmio;
use scarlet::device::manager::{DeviceManager, DriverPriority};
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::driver_initcall;
use scarlet::time;
use scarlet::vm;

const ASC_CPU_CONTROL: usize = 0x0044;
const ASC_CPU_CONTROL_START: u32 = 0x10;

const ASC_MBOX_A2I_CONTROL: usize = 0x0110;
const ASC_MBOX_I2A_CONTROL: usize = 0x0114;
const ASC_MBOX_CTRL_FULL: u32 = 1 << 16;
const ASC_MBOX_CTRL_EMPTY: u32 = 1 << 17;

const ASC_MBOX_A2I_SEND0: usize = 0x0800;
const ASC_MBOX_A2I_SEND1: usize = 0x0808;
const ASC_MBOX_I2A_RECV0: usize = 0x0830;
const ASC_MBOX_I2A_RECV1: usize = 0x0838;

const ASC_SEND_TIMEOUT_US: u64 = 1_000_000;
const ASC_POLL_DELAY_US: u64 = 1;

/// One ASC mailbox message pair.
pub struct AscMessage {
    /// Primary 64-bit payload.
    pub msg0: u64,
    /// Secondary 32-bit payload (typically endpoint ID).
    pub msg1: u32,
}

/// Apple ASC mailbox MMIO driver.
pub struct AppleAsc {
    base: usize,
}

impl AppleAsc {
    /// Create a new ASC instance from an MMIO base address.
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    /// Start the ASC IOP CPU.
    pub fn cpu_start(&self) {
        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        unsafe {
            mmio::write32(self.base + ASC_CPU_CONTROL, ASC_CPU_CONTROL_START);
        }
    }

    /// Stop the ASC IOP CPU by clearing START bit.
    pub fn cpu_stop(&self) {
        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        let ctrl = unsafe { mmio::read32(self.base + ASC_CPU_CONTROL) };
        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        unsafe {
            mmio::write32(self.base + ASC_CPU_CONTROL, ctrl & !ASC_CPU_CONTROL_START);
        }
    }

    /// Check whether there is a pending IOP->AP message.
    pub fn can_recv(&self) -> bool {
        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        let status = unsafe { mmio::read32(self.base + ASC_MBOX_I2A_CONTROL) };
        (status & ASC_MBOX_CTRL_EMPTY) == 0
    }

    /// Send one AP->IOP message.
    pub fn send(&self, msg: &AscMessage) -> Result<(), &'static str> {
        let start = time::current_time();
        loop {
            // SAFETY: `self.base` points to a mapped ASC MMIO region.
            let status = unsafe { mmio::read32(self.base + ASC_MBOX_A2I_CONTROL) };
            if (status & ASC_MBOX_CTRL_FULL) == 0 {
                break;
            }

            if time::current_time().saturating_sub(start) >= ASC_SEND_TIMEOUT_US {
                return Err("apple-asc: send mailbox full timeout");
            }

            time::udelay(ASC_POLL_DELAY_US);
        }

        // SAFETY: MMIO transaction ordering for ASC mailbox writes requires dsb ish.
        unsafe {
            dsb_ish();
        }

        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        unsafe {
            mmio::write64(self.base + ASC_MBOX_A2I_SEND0, msg.msg0);
            mmio::write64(self.base + ASC_MBOX_A2I_SEND1, msg.msg1 as u64);
        }

        Ok(())
    }

    /// Receive one IOP->AP message.
    pub fn recv(&self, msg: &mut AscMessage) -> Result<(), &'static str> {
        if !self.can_recv() {
            return Err("apple-asc: no message available");
        }

        // SAFETY: `self.base` points to a mapped ASC MMIO region.
        unsafe {
            msg.msg0 = mmio::read64(self.base + ASC_MBOX_I2A_RECV0);
            msg.msg1 = mmio::read64(self.base + ASC_MBOX_I2A_RECV1) as u32;
        }

        // SAFETY: MMIO transaction ordering for ASC mailbox reads requires dsb ish.
        unsafe {
            dsb_ish();
        }

        Ok(())
    }

    /// Receive one message with timeout in microseconds.
    pub fn recv_timeout(&self, msg: &mut AscMessage, timeout_us: u64) -> Result<(), &'static str> {
        let start = time::current_time();
        loop {
            if self.can_recv() {
                return self.recv(msg);
            }

            if time::current_time().saturating_sub(start) >= timeout_us {
                return Err("apple-asc: recv timeout");
            }

            time::udelay(ASC_POLL_DELAY_US);
        }
    }
}

/// Registry of probed ASC mailbox instances.
static ASC_REGISTRY: Mutex<Vec<Arc<AppleAsc>>> = Mutex::new(Vec::new());

/// Get a probed ASC mailbox instance by index.
pub fn get_apple_asc(id: u32) -> Option<Arc<AppleAsc>> {
    ASC_REGISTRY.lock().get(id as usize).map(Arc::clone)
}

#[inline(always)]
unsafe fn dsb_ish() {
    // SAFETY: Emits the required AArch64 barrier instruction for MMIO ordering.
    unsafe {
        asm!("dsb ish", options(nostack, nomem, preserves_flags));
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-asc: no memory resource")?;

    let paddr = resource.start;
    let size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|value| value.checked_add(1))
        .ok_or("apple-asc: invalid memory resource")?;

    let base = vm::ioremap(paddr, size).map_err(|_| "apple-asc: ioremap failed")?;
    ASC_REGISTRY.lock().push(Arc::new(AppleAsc::new(base)));

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_asc_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-asc-mailbox",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-asc-mailbox",
            "apple,asc-mailbox-v4",
            "apple,asc-mailbox",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_asc_driver);

#[used]
static SCARLET_DRIVER_APPLE_ASC_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
