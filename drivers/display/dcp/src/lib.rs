#![no_std]

//! Native Apple DCP display driver.
//!
//! The driver follows m1n1's DCP iBoot hand-off: it rebuilds the DCP and
//! display DART mappings described by `iommu-addresses`, boots RTKit, selects a
//! panel mode through `disp0-service`, and presents through DCP surface swaps.
//! Limine's framebuffer memory is not used by this driver.

extern crate alloc;

mod iomfb;

use iomfb::{BandwidthRegisters, Iomfb};

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::mem;
use core::sync::atomic::{AtomicUsize, Ordering};

use scarlet::device::graphics::output::{DisplayOutput, DisplayRegion};
use scarlet::device::graphics::{FramebufferConfig, GraphicsDevice, PixelFormat};
use scarlet::device::iommu::IommuStreamId;
use scarlet::device::manager::{DeviceManager, DriverPriority, is_probe_defer, probe_defer};
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::device::remoteproc::{RemoteProcessor, RemoteprocDmaMapper, RemoteprocError};
use scarlet::device::{Device, DeviceInfo, DeviceType};
use scarlet::mem::page::ContiguousPages;
use scarlet::object::capability::selectable::{ReadyInterest, SelectWaitOutcome, Selectable};
use scarlet::object::capability::{ControlOps, MemoryMappingOps};
use scarlet::println;
use scarlet::sync::Mutex;
use scarlet::{arch, environment, time};
use scarlet_driver_apple_asc::get_apple_asc_by_phandle;
use scarlet_driver_apple_dart::{DartDomain, DartInstance, DartPageTable, get_dart_by_phandle};
use scarlet_driver_apple_epic::EpicEndpoint;
use scarlet_driver_apple_rtkit::AppleRtkit;

const DCP_IBOOT_EP: u8 = 0x23;
const DCP_IBOOT_SUBTYPE: u16 = 0xc0;
const DCP_SERVICE_TIMEOUT_US: u64 = 5_000_000;
const DCP_STATUS_RETRIES: usize = 20;
const DCP_STATUS_RETRY_US: u64 = 100_000;
// m1n1 reserves 0x1000_0000..0x2000_0000 for its own RTKit/display handoff.
// Keep Scarlet-owned mappings in a disjoint range.
const DCP_DYNAMIC_IOVA_BASE: usize = 0x3000_0000;
const T8103_ASC_DRAM_MASK: u64 = 0xf_0000_0000;
const DCP_SCANOUT_IOVA_BASE: usize = 0x4000_0000;
const DCP_DART_FLAGS: u64 = 1;

const IBOOT_SET_SURFACE: u32 = 1;
const IBOOT_SET_POWER: u32 = 2;
const IBOOT_GET_HPD: u32 = 3;
const IBOOT_GET_TIMING_MODES: u32 = 4;
const IBOOT_GET_COLOR_MODES: u32 = 5;
const IBOOT_SET_MODE: u32 = 6;

const SURFACE_FMT_BGRA8888: u32 = 1;
const ADDR_FORMAT_PLANAR: u32 = 1;
const COLORSPACE_DISPLAY_P3: u32 = 2;
const EOTF_GAMMA_SDR: u32 = 1;

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct DcpTimingMode {
    valid: u32,
    width: u32,
    height: u32,
    fps: u32,
    _pad: [u8; 8],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct DcpColorMode {
    valid: u32,
    colorimetry: u32,
    eotf: u32,
    encoding: u32,
    bpp: u32,
    _pad: [u8; 4],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct DcpPlane {
    valid: u32,
    addr: u64,
    tile_size: u32,
    stride: u32,
    _unknown: [u32; 4],
    addr_format: u32,
    _unknown2: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct DcpLayer {
    planes: [DcpPlane; 3],
    _unknown: u32,
    plane_count: u32,
    width: u32,
    height: u32,
    surface_format: u32,
    colorspace: u32,
    eotf: u32,
    transform: u8,
    _pad: [u8; 3],
}

fn bytes_of<T>(value: &T) -> &[u8] {
    // SAFETY: the wire structs are plain repr(C, packed) values and the slice
    // does not outlive the borrowed value.
    unsafe { core::slice::from_raw_parts(value as *const T as *const u8, mem::size_of::<T>()) }
}

fn read_wire<T: Copy>(bytes: &[u8], offset: usize) -> Option<T> {
    let end = offset.checked_add(mem::size_of::<T>())?;
    if end > bytes.len() {
        return None;
    }
    // SAFETY: bounds are checked and packed wire data may be unaligned.
    Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(offset) as *const T) })
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_be_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn property_phandle(device: &PlatformDeviceInfo, name: &str) -> Option<u32> {
    read_be_u32(device.property(name)?.value(), 0)
}

fn device_phandle(device: &PlatformDeviceInfo) -> Option<u32> {
    property_phandle(device, "phandle").or_else(|| property_phandle(device, "linux,phandle"))
}

fn phandle_reg(phandle: u32, index: usize) -> Option<(usize, usize)> {
    let fdt = scarlet::device::fdt::FdtManager::get_manager().get_fdt()?;
    for node in fdt.all_nodes() {
        let node_phandle = node
            .property("phandle")
            .or_else(|| node.property("linux,phandle"))
            .and_then(|property| read_be_u32(property.value, 0));
        if node_phandle != Some(phandle) {
            continue;
        }
        let region = node.reg()?.nth(index)?;
        return Some((region.starting_address as usize, region.size.unwrap_or(0)));
    }
    None
}

fn device_clock_frequency(device: &PlatformDeviceInfo) -> u64 {
    let referenced = device
        .property("clocks")
        .and_then(|property| read_be_u32(property.value(), 0));
    if let Some(phandle) = referenced
        && let Some(fdt) = scarlet::device::fdt::FdtManager::get_manager().get_fdt()
    {
        for node in fdt.all_nodes() {
            let node_phandle = node
                .property("phandle")
                .or_else(|| node.property("linux,phandle"))
                .and_then(|property| read_be_u32(property.value, 0));
            if node_phandle == Some(phandle)
                && let Some(frequency) = node
                    .property("clock-frequency")
                    .and_then(|property| read_be_u32(property.value, 0))
            {
                return frequency as u64;
            }
        }
    }

    device
        .property("clock-frequency")
        .and_then(|property| read_be_u32(property.value(), 0))
        .unwrap_or(0) as u64
}

fn iomfb_registers(
    device: &PlatformDeviceInfo,
) -> Result<(Vec<(usize, usize)>, Option<BandwidthRegisters>), &'static str> {
    let mut registers = device
        .get_resources()
        .iter()
        .filter(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .skip(1)
        .map(|resource| {
            let size = resource
                .end
                .checked_sub(resource.start)
                .and_then(|length| length.checked_add(1))
                .ok_or("apple-dcp: invalid display register resource")?;
            Ok((resource.start, size))
        })
        .collect::<Result<Vec<_>, &'static str>>()?;

    let Some(scratch_property) = device.property("apple,bw-scratch") else {
        return Ok((registers, None));
    };
    let scratch = scratch_property.value();
    let scratch_phandle = read_be_u32(scratch, 0).ok_or("apple-dcp: invalid bw-scratch phandle")?;
    let scratch_reg = read_be_u32(scratch, 4).ok_or("apple-dcp: invalid bw-scratch reg")? as usize;
    let scratch_index =
        read_be_u32(scratch, 8).ok_or("apple-dcp: invalid bw-scratch index")? as usize;
    let scratch_offset =
        read_be_u32(scratch, 12).ok_or("apple-dcp: invalid bw-scratch offset")? as usize;
    if scratch_index != registers.len() {
        return Err("apple-dcp: unexpected bw-scratch display index");
    }
    let scratch_resource = phandle_reg(scratch_phandle, scratch_reg)
        .ok_or("apple-dcp: bw-scratch resource not found")?;
    registers.push(scratch_resource);

    let mut doorbell = 0;
    let mut doorbell_bit = 0;
    if let Some(doorbell_property) = device.property("apple,bw-doorbell") {
        let value = doorbell_property.value();
        let phandle = read_be_u32(value, 0).ok_or("apple-dcp: invalid bw-doorbell phandle")?;
        let reg = read_be_u32(value, 4).ok_or("apple-dcp: invalid bw-doorbell reg")? as usize;
        let index = read_be_u32(value, 8).ok_or("apple-dcp: invalid bw-doorbell index")? as usize;
        if index != registers.len() {
            return Err("apple-dcp: unexpected bw-doorbell display index");
        }
        let resource =
            phandle_reg(phandle, reg).ok_or("apple-dcp: bw-doorbell resource not found")?;
        doorbell = resource.0 as u64;
        registers.push(resource);
        let dcp_index = device
            .property("apple,dcp-index")
            .and_then(|property| read_be_u32(property.value(), 0))
            .unwrap_or(0);
        doorbell_bit = 2 + dcp_index;
    }

    Ok((
        registers,
        Some(BandwidthRegisters {
            scratch: scratch_resource.0.saturating_add(scratch_offset) as u64,
            doorbell,
            doorbell_bit,
        }),
    ))
}

fn iommu_spec(device: &PlatformDeviceInfo) -> Option<(u32, usize)> {
    let value = device.property("iommus")?.value();
    Some((read_be_u32(value, 0)?, read_be_u32(value, 4)? as usize))
}

fn find_display_iommu() -> Option<(u32, usize, u32)> {
    let fdt = scarlet::device::fdt::FdtManager::get_manager().get_fdt()?;
    for node in fdt.all_nodes() {
        let Some(compatible) = node.compatible() else {
            continue;
        };
        if !compatible
            .all()
            .any(|value| value == "apple,display-subsystem")
        {
            continue;
        }
        let Some(iommus) = node.property("iommus") else {
            continue;
        };
        let Some(phandle) = node
            .property("phandle")
            .or_else(|| node.property("linux,phandle"))
        else {
            continue;
        };
        return Some((
            read_be_u32(iommus.value, 0)?,
            read_be_u32(iommus.value, 4)? as usize,
            read_be_u32(phandle.value, 0)?,
        ));
    }
    None
}

fn find_piodma_iommu(dcp_phandle: u32) -> Option<(u32, usize)> {
    let fdt = scarlet::device::fdt::FdtManager::get_manager().get_fdt()?;
    for node in fdt.all_nodes() {
        let phandle = node
            .property("phandle")
            .or_else(|| node.property("linux,phandle"))
            .and_then(|property| read_be_u32(property.value, 0));
        if phandle != Some(dcp_phandle) {
            continue;
        }
        let piodma = node.children().find(|child| child.name == "piodma")?;
        let iommus = piodma.property("iommus")?;
        return Some((
            read_be_u32(iommus.value, 0)?,
            read_be_u32(iommus.value, 4)? as usize,
        ));
    }
    None
}

fn map_handoff_regions(
    table: &mut DartPageTable,
    device_phandle: u32,
) -> Result<usize, &'static str> {
    let fdt = scarlet::device::fdt::FdtManager::get_manager()
        .get_fdt()
        .ok_or("apple-dcp: FDT unavailable")?;
    let reserved = fdt
        .find_node("/reserved-memory")
        .ok_or("apple-dcp: reserved-memory missing")?;
    let mut mapped = 0usize;

    for node in reserved.children() {
        let Some(reg) = node.property("reg") else {
            continue;
        };
        let Some(addresses) = node.property("iommu-addresses") else {
            continue;
        };
        let Some(paddr) = read_be_u64(reg.value, 0) else {
            continue;
        };

        for tuple in addresses.value.chunks_exact(20) {
            if read_be_u32(tuple, 0) != Some(device_phandle) {
                continue;
            }
            let iova = read_be_u64(tuple, 4).ok_or("apple-dcp: malformed handoff IOVA")?;
            let size = read_be_u64(tuple, 12).ok_or("apple-dcp: malformed handoff size")?;
            table.map_contiguous(iova as usize, paddr as usize, size as usize, DCP_DART_FLAGS)?;
            mapped += 1;
        }
    }

    Ok(mapped)
}

struct DcpDmaMapper {
    table: Arc<Mutex<DartPageTable>>,
    dart: Arc<DartInstance>,
    next_iova: AtomicUsize,
    page_size: usize,
    dva_base: u64,
}

impl DcpDmaMapper {
    fn new(
        table: Arc<Mutex<DartPageTable>>,
        dart: Arc<DartInstance>,
        page_size: usize,
        dva_base: u64,
    ) -> Self {
        Self {
            table,
            dart,
            next_iova: AtomicUsize::new(DCP_DYNAMIC_IOVA_BASE),
            page_size,
            dva_base,
        }
    }

    fn dva_from_iova(&self, iova: usize) -> u64 {
        self.dva_base | iova as u64
    }

    fn iova_from_dva(&self, dva: u64) -> usize {
        (dva & !self.dva_base) as usize
    }
}

impl RemoteprocDmaMapper for DcpDmaMapper {
    fn alignment(&self) -> usize {
        self.page_size
    }

    fn map(&self, paddr: usize, size: usize) -> Result<u64, RemoteprocError> {
        if !paddr.is_multiple_of(self.page_size) {
            return Err(RemoteprocError::LoadFailed);
        }
        let size = size.div_ceil(self.page_size) * self.page_size;
        let iova = self.next_iova.fetch_add(size, Ordering::Relaxed);
        let mut table = self.table.lock();
        table
            .map_contiguous(iova, paddr, size, DCP_DART_FLAGS)
            .map_err(|_| RemoteprocError::LoadFailed)?;
        self.dart
            .sync_page_tables()
            .map_err(|_| RemoteprocError::LoadFailed)?;
        Ok(self.dva_from_iova(iova))
    }

    fn translate(&self, dva: u64) -> Option<usize> {
        self.table.lock().translate_iova(self.iova_from_dva(dva))
    }

    fn unmap(&self, dva: u64, size: usize) {
        let pages = size.div_ceil(self.page_size);
        let iova = self.iova_from_dva(dva);
        let mut table = self.table.lock();
        for page in 0..pages {
            table.unmap_page(iova + page * self.page_size);
        }
        drop(table);
        let _ = self.dart.sync_page_tables();
    }
}

struct DcpIboot {
    endpoint: EpicEndpoint,
    channel: u32,
}

impl DcpIboot {
    fn call(&mut self, operation: u32, payload: &[u8]) -> Result<Vec<u8>, &'static str> {
        let total = 16usize
            .checked_add(payload.len())
            .ok_or("apple-dcp: iBoot command too large")?;
        let mut command = alloc::vec![0u8; total];
        command[0..4].copy_from_slice(&operation.to_le_bytes());
        command[4..8].copy_from_slice(&(total as u32).to_le_bytes());
        command[16..].copy_from_slice(payload);

        let reply = self
            .endpoint
            .call_raw_by_channel(self.channel, DCP_IBOOT_SUBTYPE, &command)?;

        // Commands that only mutate display state return their status in the
        // EPIC command retcode and do not have to populate an iBoot rxcmd
        // header. This matches Asahi/m1n1, which only inspects the DMA response
        // for commands with a defined output payload.
        let expects_output = matches!(
            operation,
            IBOOT_GET_HPD | IBOOT_GET_TIMING_MODES | IBOOT_GET_COLOR_MODES
        );
        if reply.len() < 8 {
            return if expects_output {
                Err("apple-dcp: short iBoot reply")
            } else {
                Ok(Vec::new())
            };
        }
        let reply_len = u32::from_le_bytes(reply[4..8].try_into().unwrap()) as usize;
        if reply_len < 8 {
            return if expects_output {
                Err("apple-dcp: invalid iBoot reply length")
            } else {
                Ok(Vec::new())
            };
        }
        let reply_len = reply_len.min(reply.len());
        Ok(reply[8..reply_len].to_vec())
    }

    fn set_power(&mut self, enabled: bool) -> Result<(), &'static str> {
        self.call(IBOOT_SET_POWER, &[enabled as u8])?;
        Ok(())
    }

    fn display_status(&mut self) -> Result<(bool, u32, u32), &'static str> {
        let reply = self.call(IBOOT_GET_HPD, &[])?;
        if reply.len() < 12 {
            return Err("apple-dcp: short display status reply");
        }
        Ok((
            reply[0] != 0,
            u32::from_le_bytes(reply[4..8].try_into().unwrap()),
            u32::from_le_bytes(reply[8..12].try_into().unwrap()),
        ))
    }

    fn timing_modes(&mut self) -> Result<Vec<DcpTimingMode>, &'static str> {
        let reply = self.call(IBOOT_GET_TIMING_MODES, &[])?;
        let count = read_wire::<u32>(&reply, 0).ok_or("apple-dcp: missing timing count")?;
        let mut modes = Vec::new();
        for index in 0..count as usize {
            modes.push(
                read_wire(&reply, 4 + index * mem::size_of::<DcpTimingMode>())
                    .ok_or("apple-dcp: truncated timing modes")?,
            );
        }
        Ok(modes)
    }

    fn color_modes(&mut self) -> Result<Vec<DcpColorMode>, &'static str> {
        let reply = self.call(IBOOT_GET_COLOR_MODES, &[])?;
        let count = read_wire::<u32>(&reply, 0).ok_or("apple-dcp: missing color count")?;
        let mut modes = Vec::new();
        for index in 0..count as usize {
            modes.push(
                read_wire(&reply, 4 + index * mem::size_of::<DcpColorMode>())
                    .ok_or("apple-dcp: truncated color modes")?,
            );
        }
        Ok(modes)
    }

    fn set_mode(
        &mut self,
        timing: &DcpTimingMode,
        color: &DcpColorMode,
    ) -> Result<(), &'static str> {
        let mut payload =
            alloc::vec![0u8; mem::size_of::<DcpTimingMode>() + mem::size_of::<DcpColorMode>()];
        let timing_len = mem::size_of::<DcpTimingMode>();
        payload[..timing_len].copy_from_slice(bytes_of(timing));
        payload[timing_len..].copy_from_slice(bytes_of(color));
        self.call(IBOOT_SET_MODE, &payload)?;
        Ok(())
    }

    fn set_surface(&mut self, layer: &DcpLayer) -> Result<(), &'static str> {
        self.call(IBOOT_SET_SURFACE, bytes_of(layer))?;
        Ok(())
    }
}

fn make_layer(config: &FramebufferConfig, dva: u64) -> DcpLayer {
    let mut layer = DcpLayer::default();
    layer.planes[0].addr = dva;
    layer.planes[0].stride = config.stride;
    layer.planes[0].addr_format = ADDR_FORMAT_PLANAR;
    layer.plane_count = 1;
    layer.width = config.width;
    layer.height = config.height;
    layer.surface_format = SURFACE_FMT_BGRA8888;
    layer.colorspace = COLORSPACE_DISPLAY_P3;
    layer.eotf = EOTF_GAMMA_SDR;
    layer
}

fn choose_timing(modes: &[DcpTimingMode]) -> Option<DcpTimingMode> {
    modes
        .iter()
        .copied()
        .filter(|mode| mode.valid != 0 && mode.width != 0 && mode.height != 0)
        .max_by_key(|mode| {
            let refresh_ok = mode.fps <= (60 << 16);
            (refresh_ok, mode.width as u64 * mode.height as u64, mode.fps)
        })
}

fn choose_color(modes: &[DcpColorMode]) -> Option<DcpColorMode> {
    modes
        .iter()
        .copied()
        .filter(|mode| mode.valid != 0)
        .max_by_key(|mode| (mode.bpp <= 32, mode.bpp))
}

struct DcpState {
    _iboot: DcpIboot,
    _iomfb: Iomfb,
}

pub struct AppleDcpGraphics {
    config: FramebufferConfig,
    scanout: ContiguousPages,
    _state: Mutex<DcpState>,
    _rtkit: Arc<AppleRtkit>,
    _dcp_table: Arc<Mutex<DartPageTable>>,
    _display_table: Arc<Mutex<DartPageTable>>,
    _piodma_domain: Arc<DartDomain>,
}

impl Device for AppleDcpGraphics {
    fn device_type(&self) -> DeviceType {
        DeviceType::Graphics
    }

    fn name(&self) -> &'static str {
        "apple-dcp"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_graphics_device(&self) -> Option<&dyn GraphicsDevice> {
        Some(self)
    }
}

impl GraphicsDevice for AppleDcpGraphics {
    fn get_display_name(&self) -> &'static str {
        "apple-dcp-internal-panel"
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        Ok(self.config.clone())
    }

    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        Ok(self.scanout.as_paddr())
    }

    fn present_framebuffer_region(
        &self,
        config: &FramebufferConfig,
        physical_addr: usize,
        _region: DisplayRegion,
    ) -> Result<(), &'static str> {
        if physical_addr != self.scanout.as_paddr()
            || config.width != self.config.width
            || config.height != self.config.height
            || config.stride != self.config.stride
            || config.format != self.config.format
        {
            return Err("apple-dcp: framebuffer does not match scanout surface");
        }

        // Compatibility path: SWS writes directly into the one surface that
        // iBoot installed for scanout. Publish those device-memory writes, but
        // do not copy the frame or submit an IOMFB page flip.
        arch::io_wmb();
        Ok(())
    }

    fn init_graphics(&self) -> Result<(), &'static str> {
        Ok(())
    }

    fn get_outputs(&self) -> Vec<&dyn DisplayOutput> {
        Vec::new()
    }
}

impl ControlOps for AppleDcpGraphics {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("apple-dcp: control command unsupported")
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        Vec::new()
    }
}

impl MemoryMappingOps for AppleDcpGraphics {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<scarlet::object::capability::MemoryMappingInfo, &'static str> {
        Err("apple-dcp: map /dev/display0 instead")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for AppleDcpGraphics {
    fn current_ready(
        &self,
        _interest: ReadyInterest,
    ) -> scarlet::object::capability::selectable::ReadySet {
        scarlet::object::capability::selectable::ReadySet::none()
    }

    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut scarlet::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

fn probe_deferred(message: &'static str) -> Result<(), &'static str> {
    println!("[apple-dcp] {}, deferring", message);
    let result = probe_defer();
    if let Err(error) = result {
        debug_assert!(is_probe_defer(error));
    }
    result
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let dcp_phandle = device_phandle(device).ok_or("apple-dcp: missing phandle")?;
    let (dcp_dart_phandle, dcp_stream) =
        iommu_spec(device).ok_or("apple-dcp: missing DCP IOMMU")?;
    let (display_dart_phandle, display_stream, display_phandle) =
        find_display_iommu().ok_or("apple-dcp: display-subsystem IOMMU missing")?;
    let (piodma_dart_phandle, piodma_stream) =
        find_piodma_iommu(dcp_phandle).ok_or("apple-dcp: PIODMA IOMMU missing")?;

    let Some(dcp_dart) = get_dart_by_phandle(dcp_dart_phandle) else {
        return probe_deferred("DCP DART is not ready");
    };
    let Some(display_dart) = get_dart_by_phandle(display_dart_phandle) else {
        return probe_deferred("display DART is not ready");
    };
    let Some(piodma_dart) = get_dart_by_phandle(piodma_dart_phandle) else {
        return probe_deferred("PIODMA DART is not ready");
    };

    let dcp_root = dcp_dart
        .ttbr_paddr(dcp_stream)
        .ok_or("apple-dcp: DCP DART has no valid TTBR")?;
    let display_root = display_dart
        .ttbr_paddr(display_stream)
        .ok_or("apple-dcp: display DART has no valid TTBR")?;
    let dcp_table = Arc::new(Mutex::new(DartPageTable::wrap_existing(
        dcp_root,
        dcp_dart.page_shift(),
    )?));
    let display_table = Arc::new(Mutex::new(DartPageTable::wrap_existing(
        display_root,
        display_dart.page_shift(),
    )?));
    let piodma_stream_id =
        u32::try_from(piodma_stream).map_err(|_| "apple-dcp: PIODMA stream ID out of range")?;
    let piodma_domain = Arc::new(
        DartDomain::wrap_existing(
            Arc::clone(&piodma_dart),
            IommuStreamId {
                id: piodma_stream_id,
                substream_id: None,
            },
        )
        .map_err(|_| "apple-dcp: PIODMA firmware page table unavailable")?,
    );

    let dcp_handoff = map_handoff_regions(&mut dcp_table.lock(), dcp_phandle)?;
    let display_handoff = map_handoff_regions(&mut display_table.lock(), display_phandle)?;
    if dcp_handoff == 0 {
        return Err("apple-dcp: no firmware handoff mappings");
    }

    dcp_dart
        .sync_page_tables()
        .map_err(|_| "apple-dcp: DCP DART sync failed")?;
    display_dart
        .sync_page_tables()
        .map_err(|_| "apple-dcp: display DART sync failed")?;
    let mailbox_phandle = property_phandle(device, "mboxes")
        .or_else(|| property_phandle(device, "mailboxes"))
        .ok_or("apple-dcp: missing ASC mailbox")?;
    let Some(asc) = get_apple_asc_by_phandle(mailbox_phandle) else {
        return probe_deferred("ASC mailbox is not ready");
    };

    let page_size = 1usize << dcp_dart.page_shift();
    let dva_base = device
        .property("apple,asc-dram-mask")
        .or_else(|| device.property("asc-dram-mask"))
        .and_then(|property| read_be_u64(property.value(), 0))
        .unwrap_or_else(|| {
            if device.compatible().contains(&"apple,t8103-dcp") {
                T8103_ASC_DRAM_MASK
            } else {
                0
            }
        });
    let mapper = Arc::new(DcpDmaMapper::new(
        Arc::clone(&dcp_table),
        Arc::clone(&dcp_dart),
        page_size,
        dva_base,
    ));
    let rtkit = Arc::new(AppleRtkit::new_with_dma_mapper(asc, mapper));
    // Match Asahi Linux's afk_start() ordering: complete the RTKit power
    // handshake first, then start the application endpoint immediately before
    // sending AFK INIT. Some DCP firmware only begins EPIC publication when
    // the endpoint transitions to started in this phase.
    rtkit.wake()?;
    rtkit.start_ep(DCP_IBOOT_EP)?;
    let remoteproc: Arc<dyn RemoteProcessor> = rtkit.clone();
    let mut endpoint = EpicEndpoint::new(remoteproc, DCP_IBOOT_EP)?;
    endpoint.wait_for_services(1, DCP_SERVICE_TIMEOUT_US)?;
    let channel = endpoint
        .find_service("disp0")
        .map(|service| service.channel)
        .or_else(|| endpoint.first_service_channel())
        .ok_or("apple-dcp: disp0-service not announced")?;
    let mut iboot = DcpIboot { endpoint, channel };

    iboot.set_power(true)?;
    let mut status = (false, 0, 0);
    for _ in 0..DCP_STATUS_RETRIES {
        status = iboot.display_status()?;
        if status.0 && status.1 > 0 && status.2 > 0 {
            break;
        }
        time::udelay(DCP_STATUS_RETRY_US);
    }
    if !status.0 {
        return Err("apple-dcp: internal panel is not connected");
    }

    let timing = choose_timing(&iboot.timing_modes()?).ok_or("apple-dcp: no usable timing mode")?;
    let color = choose_color(&iboot.color_modes()?).ok_or("apple-dcp: no usable color mode")?;
    iboot.set_mode(&timing, &color)?;

    let config = FramebufferConfig {
        width: timing.width,
        height: timing.height,
        format: PixelFormat::BGRA8888,
        stride: timing.width.saturating_mul(4),
    };
    let visible_size = config.size();
    let allocation_size = visible_size
        .checked_add(24 * page_size)
        .ok_or("apple-dcp: scanout size overflow")?
        .div_ceil(page_size)
        * page_size;
    let scanout_pages = allocation_size.div_ceil(environment::PAGE_SIZE);
    let scanout0 = ContiguousPages::new_aligned(scanout_pages, page_size)
        .ok_or("apple-dcp: scanout 0 allocation failed")?;

    // SAFETY: the page allocation is live and covers `allocation_size` bytes.
    unsafe {
        core::ptr::write_bytes(scanout0.as_ptr() as *mut u8, 0, allocation_size);
    }
    arch::clean_dcache_to_poc_range(scanout0.as_ptr() as usize, allocation_size);

    let scanout_iova = DCP_SCANOUT_IOVA_BASE;
    let scanout_dva = dva_base | scanout_iova as u64;
    dcp_table.lock().map_contiguous(
        scanout_iova,
        scanout0.as_paddr(),
        allocation_size,
        DCP_DART_FLAGS,
    )?;
    display_table.lock().map_contiguous(
        scanout_iova,
        scanout0.as_paddr(),
        allocation_size,
        DCP_DART_FLAGS,
    )?;
    dcp_dart
        .sync_page_tables()
        .map_err(|_| "apple-dcp: DCP scanout DART sync failed")?;
    display_dart
        .sync_page_tables()
        .map_err(|_| "apple-dcp: display scanout DART sync failed")?;

    arch::io_wmb();
    iboot.set_surface(&make_layer(&config, scanout_dva))?;
    let (registers, bandwidth) = iomfb_registers(device)?;
    let clock_frequency = device_clock_frequency(device);
    let firmware_compat = device
        .property("apple,firmware-compat")
        .ok_or("apple-dcp: missing apple,firmware-compat")?;
    let firmware_compat = firmware_compat.value();
    let firmware_major =
        read_be_u32(firmware_compat, 0).ok_or("apple-dcp: invalid firmware compatibility")?;
    let firmware_minor =
        read_be_u32(firmware_compat, 4).ok_or("apple-dcp: invalid firmware compatibility")?;
    let firmware_patch =
        read_be_u32(firmware_compat, 8).ok_or("apple-dcp: invalid firmware compatibility")?;
    let firmware_12_3 = match (firmware_major, firmware_minor, firmware_patch) {
        (12, 3, 0) => true,
        (13, 3, 0) | (13, 5, 0) => false,
        _ => return Err("apple-dcp: unsupported firmware compatibility"),
    };
    println!(
        "[apple-dcp] IOMFB firmware compatibility {}.{}.{}",
        firmware_major, firmware_minor, firmware_patch
    );
    let mut iomfb = Iomfb::new(
        rtkit.clone(),
        registers,
        bandwidth,
        clock_frequency,
        Arc::clone(&piodma_domain),
        firmware_12_3,
    )?;
    iomfb.start()?;
    iomfb.power_on()?;

    let graphics = Arc::new(AppleDcpGraphics {
        config: config.clone(),
        scanout: scanout0,
        _state: Mutex::new(DcpState {
            _iboot: iboot,
            _iomfb: iomfb,
        }),
        _rtkit: rtkit,
        _dcp_table: dcp_table,
        _display_table: display_table,
        _piodma_domain: piodma_domain,
    });
    let device_id = DeviceManager::get_manager()
        .register_device_with_name(alloc::string::String::from("apple-dcp"), graphics.clone());
    scarlet::device::graphics::manager::GraphicsManager::get_manager()
        .register_framebuffer_from_device(device_id, graphics)?;

    if scarlet::earlyfb::is_initialized() {
        scarlet::earlyfb::deactivate();
    }

    println!(
        "[apple-dcp] native panel {}x{} @ {}.{:02} Hz, handoff maps dcp={} display={}",
        config.width,
        config.height,
        timing.fps >> 16,
        ((timing.fps & 0xffff) * 100 + 0x7fff) >> 16,
        dcp_handoff,
        display_handoff
    );
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-dcp",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-dcp", "apple,dcp"],
    );
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_DCP_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
