#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
    device::{
        DeviceInfo,
        dma::{DmaChannel, DmaController, DmaCyclicConfig, DmaError, DmaSpec},
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
};

const APPLE_ADMAC_DMA_CELLS: usize = 1;

struct AppleAdmacInner {
    base: usize,
    size: usize,
    channel_count: usize,
    channel_in_use: Mutex<Vec<bool>>,
}

/// Apple ADMAC DMA controller.
pub struct AppleAdmac {
    inner: Arc<AppleAdmacInner>,
}

impl AppleAdmac {
    /// Create an ADMAC controller wrapper.
    ///
    /// # Arguments
    ///
    /// * `base` - Kernel virtual address of the ADMAC MMIO region.
    /// * `size` - Size of the mapped MMIO region in bytes.
    /// * `channel_count` - Number of channels reported by firmware.
    ///
    /// # Returns
    ///
    /// A new ADMAC controller instance.
    pub fn new(base: usize, size: usize, channel_count: usize) -> Self {
        Self {
            inner: Arc::new(AppleAdmacInner {
                base,
                size,
                channel_count,
                channel_in_use: Mutex::new(alloc::vec![false; channel_count]),
            }),
        }
    }
}

impl DmaController for AppleAdmac {
    fn name(&self) -> &'static str {
        "apple-admac"
    }

    fn dma_cells(&self) -> usize {
        APPLE_ADMAC_DMA_CELLS
    }

    fn request_channel(&self, spec: &DmaSpec) -> Result<Arc<dyn DmaChannel>, DmaError> {
        if spec.cells.len() != APPLE_ADMAC_DMA_CELLS {
            return Err(DmaError::InvalidSpec);
        }

        let index = spec.cells[0] as usize;
        if index >= self.inner.channel_count {
            return Err(DmaError::ChannelNotFound);
        }

        let mut in_use = self.inner.channel_in_use.lock();
        if in_use[index] {
            return Err(DmaError::ChannelBusy);
        }
        in_use[index] = true;
        drop(in_use);

        Ok(Arc::new(AppleAdmacChannel {
            inner: self.inner.clone(),
            index,
            prepared: Mutex::new(None),
            running: Mutex::new(false),
        }))
    }
}

struct AppleAdmacChannel {
    inner: Arc<AppleAdmacInner>,
    index: usize,
    prepared: Mutex<Option<DmaCyclicConfig>>,
    running: Mutex<bool>,
}

impl Drop for AppleAdmacChannel {
    fn drop(&mut self) {
        let mut in_use = self.inner.channel_in_use.lock();
        if self.index < in_use.len() {
            in_use[self.index] = false;
        }
    }
}

impl DmaChannel for AppleAdmacChannel {
    fn name(&self) -> &'static str {
        "apple-admac-channel"
    }

    fn prepare_cyclic(&self, config: DmaCyclicConfig) -> Result<(), DmaError> {
        config.validate()?;
        *self.prepared.lock() = Some(config);
        Ok(())
    }

    fn start(&self) -> Result<(), DmaError> {
        if self.prepared.lock().is_none() {
            return Err(DmaError::NotPrepared);
        }

        Err(DmaError::Unsupported)
    }

    fn stop(&self) -> Result<(), DmaError> {
        *self.running.lock() = false;
        Ok(())
    }

    fn is_running(&self) -> bool {
        *self.running.lock()
    }
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|value| value as u32)
        .ok_or("apple-admac: missing phandle")
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-admac: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;
    let base = scarlet::vm::ioremap(paddr, size).map_err(|_| "apple-admac: ioremap failed")?;
    let phandle = read_phandle(device)?;
    let channel_count = device
        .property("dma-channels")
        .and_then(|property| property.as_usize())
        .ok_or("apple-admac: missing dma-channels")?;
    if channel_count == 0 {
        return Err("apple-admac: invalid dma-channels");
    }

    let dma_cells = device
        .property("#dma-cells")
        .and_then(|property| property.as_usize())
        .unwrap_or(APPLE_ADMAC_DMA_CELLS);

    if dma_cells != APPLE_ADMAC_DMA_CELLS {
        return Err("apple-admac: unsupported #dma-cells");
    }

    let controller = Arc::new(AppleAdmac::new(base, size, channel_count));
    DeviceManager::get_manager().register_dma_controller(phandle, controller);

    early_println!(
        "[apple-admac] registered {} at paddr={:#x}, base={:#x}, channels={}",
        device.name(),
        paddr,
        base,
        channel_count
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-admac",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-admac",
            "apple,t8112-admac",
            "apple,t6000-admac",
            "apple,t6020-admac",
            "apple,admac",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_ADMAC_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
