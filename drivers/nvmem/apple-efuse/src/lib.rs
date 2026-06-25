#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::arch::mmio;
use scarlet::device::manager::{DeviceManager, DriverPriority};
use scarlet::device::nvmem::{NvmemError, NvmemProvider};
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::early_println;
use scarlet::vm;

/// Bitfield description within an Apple eFuse word.
#[derive(Debug, Clone)]
pub struct EfuseCell {
    /// Diagnostic cell name.
    pub name: String,
    /// Byte offset of the containing eFuse word.
    pub offset: usize,
    /// First bit of the value within the containing word.
    pub bit_offset: u32,
    /// Number of bits in the value.
    pub bit_count: u32,
}

impl EfuseCell {
    /// Extract this cell from a raw eFuse word.
    ///
    /// # Arguments
    ///
    /// * `word` - Raw 32-bit eFuse word read from hardware.
    ///
    /// # Returns
    ///
    /// The cell value shifted down to bit 0.
    pub fn extract(&self, word: u32) -> u32 {
        let mask = (1u32 << self.bit_count) - 1;
        (word >> self.bit_offset) & mask
    }
}

/// Apple eFuse MMIO-backed NVMEM provider.
pub struct AppleEfuse {
    base: usize,
    size: usize,
}

impl AppleEfuse {
    fn new(base: usize, size: usize) -> Self {
        Self { base, size }
    }

    /// Read one 32-bit eFuse word.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset within the mapped eFuse MMIO region.
    ///
    /// # Returns
    ///
    /// Raw 32-bit word returned by the eFuse controller.
    pub fn read32(&self, offset: usize) -> u32 {
        // SAFETY: `self.base + offset` points to a mapped EFUSE MMIO region.
        unsafe { mmio::read32(self.base + offset) }
    }

    /// Read and extract a bitfield cell.
    ///
    /// # Arguments
    ///
    /// * `cell` - Cell description to read.
    ///
    /// # Returns
    ///
    /// Extracted cell value.
    pub fn read_cell(&self, cell: &EfuseCell) -> u32 {
        cell.extract(self.read32(cell.offset))
    }

    fn check_range(&self, offset: usize, len: usize) -> Result<(), NvmemError> {
        let end = offset.checked_add(len).ok_or(NvmemError::OutOfRange)?;
        if end > self.size {
            return Err(NvmemError::OutOfRange);
        }

        Ok(())
    }
}

impl NvmemProvider for AppleEfuse {
    fn name(&self) -> &'static str {
        "apple-efuse"
    }

    fn size(&self) -> usize {
        self.size
    }

    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<(), NvmemError> {
        self.check_range(offset, buf.len())?;

        for (index, byte) in buf.iter_mut().enumerate() {
            let absolute = offset + index;
            let word_offset = absolute & !0x3;
            let byte_index = absolute & 0x3;
            let word = self.read32(word_offset).to_le_bytes();
            *byte = word[byte_index];
        }

        Ok(())
    }

    fn write(&self, _offset: usize, _buf: &[u8]) -> Result<(), NvmemError> {
        Err(NvmemError::NotSupported)
    }
}

static EFUSE_REGISTRY: Mutex<Vec<Arc<AppleEfuse>>> = Mutex::new(Vec::new());

/// Return a probed Apple eFuse provider by registration index.
///
/// # Arguments
///
/// * `id` - Zero-based provider index in probe order.
///
/// # Returns
///
/// Provider reference when `id` exists.
pub fn get_apple_efuse(id: u32) -> Option<Arc<AppleEfuse>> {
    EFUSE_REGISTRY.lock().get(id as usize).map(Arc::clone)
}

fn device_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|phandle| phandle as u32)
        .ok_or("apple-efuse: missing phandle")
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .find(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-efuse: no memory resource")?;

    let paddr = resource.start;
    let size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|v| v.checked_add(1))
        .ok_or("apple-efuse: invalid memory resource")?;

    let base = vm::ioremap(paddr, size).map_err(|_| "apple-efuse: ioremap failed")?;
    let phandle = device_phandle(device)?;

    early_println!("[apple-efuse] probed at {:#x} ({} bytes)", paddr, size);

    let provider = Arc::new(AppleEfuse::new(base, size));
    EFUSE_REGISTRY.lock().push(provider.clone());
    DeviceManager::get_manager().register_nvmem_provider(phandle, provider);

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_efuse_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-efuse",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-efuses", "apple,t6000-efuses", "apple,efuses"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_efuse_driver);

#[used]
static SCARLET_DRIVER_APPLE_EFUSE_ANCHOR: fn() = force_link;

/// Force the linker to keep the apple-efuse driver object.
#[inline(never)]
pub fn force_link() {}
