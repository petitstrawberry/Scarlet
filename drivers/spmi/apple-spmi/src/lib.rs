#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use scarlet::arch::mmio;
use scarlet::device::DeviceInfo;
use scarlet::device::fdt::FdtManager;
use scarlet::device::manager::{DeviceManager, DriverPriority};
use scarlet::device::nvmem::{NvmemError, NvmemProvider};
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::early_println;
use scarlet::sync::Mutex;
use scarlet::time;
use scarlet::vm;

const SPMI_STATUS_REG: usize = 0x0;
const SPMI_CMD_REG: usize = 0x4;
const SPMI_RSP_REG: usize = 0x8;

const SPMI_RX_FIFO_EMPTY: u32 = 1 << 24;
const SPMI_CMD_EXT_READL: u8 = 0x38;
const SPMI_MAX_TRANSFER: usize = 16;
const SPMI_TIMEOUT_US: u64 = 50_000;
const SPMI_POLL_DELAY_US: u64 = 10;

/// Apple SPMI controller MMIO access.
pub struct AppleSpmi {
    base: usize,
}

impl AppleSpmi {
    fn new(base: usize) -> Self {
        Self { base }
    }

    #[inline]
    fn pack_cmd(opc: u8, sid: u8, saddr: u16, len: usize) -> u32 {
        (opc as u32)
            | ((sid as u32) << 8)
            | ((saddr as u32) << 16)
            | ((len as u32 - 1) & 0xf)
            | (1 << 15)
    }

    fn wait_rx_not_empty(&self) -> Result<(), NvmemError> {
        let start = time::current_time();
        loop {
            // SAFETY: `self.base` points to a mapped Apple SPMI MMIO region.
            let status = unsafe { mmio::read32(self.base + SPMI_STATUS_REG) };
            if (status & SPMI_RX_FIFO_EMPTY) == 0 {
                return Ok(());
            }

            if time::current_time().saturating_sub(start) >= SPMI_TIMEOUT_US {
                return Err(NvmemError::HardwareError);
            }

            time::udelay(SPMI_POLL_DELAY_US);
        }
    }

    fn ext_read(&self, sid: u8, addr: u16, buf: &mut [u8]) -> Result<(), NvmemError> {
        if buf.is_empty() || buf.len() > SPMI_MAX_TRANSFER {
            return Err(NvmemError::OutOfRange);
        }

        let cmd = Self::pack_cmd(SPMI_CMD_EXT_READL, sid, addr, buf.len());
        // SAFETY: `self.base` points to a mapped Apple SPMI MMIO region.
        unsafe {
            mmio::write32(self.base + SPMI_CMD_REG, cmd);
        }

        self.wait_rx_not_empty()?;

        // SAFETY: `self.base` points to a mapped Apple SPMI MMIO region.
        unsafe {
            let _status = mmio::read32(self.base + SPMI_RSP_REG);
        }

        for chunk in buf.chunks_mut(4) {
            // SAFETY: `self.base` points to a mapped Apple SPMI MMIO region.
            let rsp = unsafe { mmio::read32(self.base + SPMI_RSP_REG) }.to_le_bytes();
            chunk.copy_from_slice(&rsp[..chunk.len()]);
        }

        Ok(())
    }
}

/// Apple SPMI-backed NVMEM provider.
pub struct AppleSpmiNvmem {
    spmi: Arc<AppleSpmi>,
    sid: u8,
    base_addr: u16,
    size: usize,
}

impl AppleSpmiNvmem {
    fn new(spmi: Arc<AppleSpmi>, sid: u8, base_addr: u16, size: usize) -> Self {
        Self {
            spmi,
            sid,
            base_addr,
            size,
        }
    }

    fn check_range(&self, offset: usize, len: usize) -> Result<(), NvmemError> {
        let end = offset.checked_add(len).ok_or(NvmemError::OutOfRange)?;
        if end > self.size {
            return Err(NvmemError::OutOfRange);
        }

        Ok(())
    }
}

impl NvmemProvider for AppleSpmiNvmem {
    fn name(&self) -> &'static str {
        "apple-spmi-nvmem"
    }

    fn size(&self) -> usize {
        self.size
    }

    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<(), NvmemError> {
        self.check_range(offset, buf.len())?;

        let mut done = 0usize;
        while done < buf.len() {
            let chunk_len = core::cmp::min(SPMI_MAX_TRANSFER, buf.len() - done);
            let relative = offset
                .checked_add(done)
                .and_then(|value| u16::try_from(value).ok())
                .ok_or(NvmemError::OutOfRange)?;
            let absolute = self
                .base_addr
                .checked_add(relative)
                .ok_or(NvmemError::OutOfRange)?;
            self.spmi
                .ext_read(self.sid, absolute, &mut buf[done..done + chunk_len])?;
            done += chunk_len;
        }

        Ok(())
    }

    fn write(&self, _offset: usize, _buf: &[u8]) -> Result<(), NvmemError> {
        Err(NvmemError::NotSupported)
    }
}

static SPMI_REGISTRY: Mutex<Vec<Arc<AppleSpmi>>> = Mutex::new(Vec::new());

fn read_be_u32_cells(bytes: &[u8]) -> Option<Vec<u32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }

    let mut cells = Vec::new();
    for chunk in bytes.chunks_exact(4) {
        cells.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Some(cells)
}

fn node_has_compatible(node: &fdt::node::FdtNode<'_, '_>, compatible: &str) -> bool {
    node.compatible()
        .is_some_and(|compatibles| compatibles.all().any(|entry| entry == compatible))
}

fn node_matches_platform_device(
    node: &fdt::node::FdtNode<'_, '_>,
    device: &PlatformDeviceInfo,
) -> bool {
    if node.name != device.name() {
        return false;
    }

    node.compatible().is_some_and(|compatibles| {
        compatibles
            .all()
            .any(|entry| device.compatible().contains(&entry))
    })
}

fn find_platform_node<'a>(
    fdt: &'a fdt::Fdt<'a>,
    device: &PlatformDeviceInfo,
) -> Option<fdt::node::FdtNode<'a, 'a>> {
    let mut stack = Vec::new();
    stack.push(fdt.find_node("/")?);

    while let Some(node) = stack.pop() {
        if node_matches_platform_device(&node, device) {
            return Some(node);
        }

        for child in node.children() {
            stack.push(child);
        }
    }

    None
}

fn first_reg_cells(node: &fdt::node::FdtNode<'_, '_>) -> Result<Vec<u32>, &'static str> {
    let reg = node
        .property("reg")
        .ok_or("apple-spmi: missing reg property")?;
    read_be_u32_cells(reg.value).ok_or("apple-spmi: malformed reg property")
}

fn register_nvmem_provider(
    spmi: &Arc<AppleSpmi>,
    sid: u8,
    node: &fdt::node::FdtNode<'_, '_>,
    base_addr: u16,
    size: usize,
) {
    let manager = DeviceManager::get_manager();
    let phandle = manager.phandle_for_fdt_node(node);
    let provider = Arc::new(AppleSpmiNvmem::new(Arc::clone(spmi), sid, base_addr, size));
    manager.register_nvmem_provider(phandle, provider);
    early_println!(
        "[apple-spmi] registered nvmem {} sid={:#x} base={:#x} size={:#x} phandle={:#x}",
        node.name,
        sid,
        base_addr,
        size,
        phandle
    );
}

fn register_pmu_nvmem_nodes(spmi: &Arc<AppleSpmi>, controller_node: &fdt::node::FdtNode<'_, '_>) {
    for pmu in controller_node.children() {
        let Ok(reg_cells) = first_reg_cells(&pmu) else {
            continue;
        };
        let Some(&sid_cell) = reg_cells.first() else {
            continue;
        };
        let sid = sid_cell as u8;

        if node_has_compatible(&pmu, "apple,spmi-nvmem") {
            register_nvmem_provider(spmi, sid, &pmu, 0, 0xffff);
        }

        for child in pmu.children() {
            if !node_has_compatible(&child, "apple,spmi-pmu-nvmem") {
                continue;
            }

            let Ok(reg_cells) = first_reg_cells(&child) else {
                continue;
            };
            if reg_cells.len() < 2 {
                continue;
            }

            register_nvmem_provider(
                spmi,
                sid,
                &child,
                reg_cells[0] as u16,
                reg_cells[1] as usize,
            );
        }
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-spmi: no memory resource")?;
    let paddr = resource.start;
    let size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|value| value.checked_add(1))
        .ok_or("apple-spmi: invalid memory resource")?;
    let base = vm::ioremap(paddr, size).map_err(|_| "apple-spmi: ioremap failed")?;
    let spmi = Arc::new(AppleSpmi::new(base));

    SPMI_REGISTRY.lock().push(Arc::clone(&spmi));

    let fdt = FdtManager::get_manager()
        .get_fdt()
        .ok_or("apple-spmi: FDT unavailable")?;
    let controller_node =
        find_platform_node(fdt, device).ok_or("apple-spmi: FDT node not found")?;
    register_pmu_nvmem_nodes(&spmi, &controller_node);

    early_println!("[apple-spmi] probed {} paddr={:#x}", device.name(), paddr);
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_spmi_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-spmi",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-spmi", "apple,spmi"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_spmi_driver);

#[used]
static SCARLET_DRIVER_APPLE_SPMI_ANCHOR: fn() = force_link;

/// Force the linker to keep the apple-spmi driver object.
#[inline(never)]
pub fn force_link() {}
