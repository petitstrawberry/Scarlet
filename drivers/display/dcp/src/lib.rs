#![no_std]

//! Native Apple DCP display driver.
//!
//! The driver follows m1n1's DCP iBoot hand-off: it rebuilds the DCP and
//! display DART mappings described by `iommu-addresses`, boots RTKit, selects a
//! panel mode through `disp0-service`, and presents through DCP surface swaps.
//! Limine's framebuffer memory is not used by this driver.

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::mem;
use core::sync::atomic::{AtomicUsize, Ordering};

use scarlet::device::graphics::output::{DisplayOutput, DisplayRegion};
use scarlet::device::graphics::{FramebufferConfig, GraphicsDevice, PixelFormat};
use scarlet::device::manager::{DeviceManager, DriverPriority, is_probe_defer, probe_defer};
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::device::remoteproc::{RemoteProcessor, RemoteprocDmaMapper, RemoteprocError};
use scarlet::device::{Device, DeviceType};
use scarlet::early_println;
use scarlet::mem::page::ContiguousPages;
use scarlet::object::capability::selectable::{ReadyInterest, SelectWaitOutcome, Selectable};
use scarlet::object::capability::{ControlOps, MemoryMappingOps};
use scarlet::sync::Mutex;
use scarlet::{arch, environment, time};
use scarlet_driver_apple_asc::get_apple_asc_by_phandle;
use scarlet_driver_apple_dart::{DartInstance, DartPageTable, get_dart_by_phandle};
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
const DCP_SCANOUT_IOVA_BASE: usize = 0x4000_0000;
const DCP_DART_FLAGS: u64 = 1;

const IBOOT_SET_SURFACE: u32 = 1;
const IBOOT_SET_POWER: u32 = 2;
const IBOOT_GET_HPD: u32 = 3;
const IBOOT_GET_TIMING_MODES: u32 = 4;
const IBOOT_GET_COLOR_MODES: u32 = 5;
const IBOOT_SET_MODE: u32 = 6;
const IBOOT_SWAP_BEGIN: u32 = 15;
const IBOOT_SWAP_SET_LAYER: u32 = 16;
const IBOOT_SWAP_END: u32 = 18;

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

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct DcpRect {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
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
    stream: usize,
    next_iova: AtomicUsize,
    page_size: usize,
}

impl DcpDmaMapper {
    fn new(
        table: Arc<Mutex<DartPageTable>>,
        dart: Arc<DartInstance>,
        stream: usize,
        page_size: usize,
    ) -> Self {
        Self {
            table,
            dart,
            stream,
            next_iova: AtomicUsize::new(DCP_DYNAMIC_IOVA_BASE),
            page_size,
        }
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
            .enable_translation(self.stream, table.root_paddr(), table.translation_levels());
        Ok(iova as u64)
    }

    fn unmap(&self, dva: u64, size: usize) {
        let pages = size.div_ceil(self.page_size);
        let mut table = self.table.lock();
        for page in 0..pages {
            table.unmap_page(dva as usize + page * self.page_size);
        }
    }
}

struct DcpIboot {
    endpoint: EpicEndpoint,
    channel: u32,
    firmware_v13: bool,
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
            IBOOT_GET_HPD | IBOOT_GET_TIMING_MODES | IBOOT_GET_COLOR_MODES | IBOOT_SWAP_BEGIN
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

    fn swap(&mut self, layer: &DcpLayer, width: u32, height: u32) -> Result<(), &'static str> {
        self.call(IBOOT_SWAP_BEGIN, &[])?;

        let extra = if self.firmware_v13 { 8 } else { 0 };
        let layer_offset = 8;
        let rect_offset = layer_offset + mem::size_of::<DcpLayer>() + extra;
        let mut payload = alloc::vec![0u8; rect_offset + 2 * mem::size_of::<DcpRect>() + 4];
        payload[4..8].copy_from_slice(&0u32.to_le_bytes());
        payload[layer_offset..layer_offset + mem::size_of::<DcpLayer>()]
            .copy_from_slice(bytes_of(layer));
        let rect = DcpRect {
            width,
            height,
            x: 0,
            y: 0,
        };
        payload[rect_offset..rect_offset + mem::size_of::<DcpRect>()]
            .copy_from_slice(bytes_of(&rect));
        payload
            [rect_offset + mem::size_of::<DcpRect>()..rect_offset + 2 * mem::size_of::<DcpRect>()]
            .copy_from_slice(bytes_of(&rect));
        self.call(IBOOT_SWAP_SET_LAYER, &payload)?;
        self.call(IBOOT_SWAP_END, &[0; 12])?;
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
    iboot: DcpIboot,
    front: usize,
    frame_delay_us: u64,
}

pub struct AppleDcpGraphics {
    config: FramebufferConfig,
    render: ContiguousPages,
    scanout: [ContiguousPages; 2],
    scanout_dva: [u64; 2],
    state: Mutex<DcpState>,
    _rtkit: Arc<AppleRtkit>,
    _dcp_table: Arc<Mutex<DartPageTable>>,
    _display_table: Arc<Mutex<DartPageTable>>,
}

impl AppleDcpGraphics {
    fn copy_region(&self, source_paddr: usize, destination_paddr: usize, region: DisplayRegion) {
        let bytes_per_pixel = self.config.format.bytes_per_pixel();
        let row_bytes = region.width as usize * bytes_per_pixel;
        let source = scarlet::vm::phys_to_virt(source_paddr);
        let destination = scarlet::vm::phys_to_virt(destination_paddr);
        for row in 0..region.height as usize {
            let offset = (region.y as usize + row) * self.config.stride as usize
                + region.x as usize * bytes_per_pixel;
            // SAFETY: the caller clips the region to the configured surface and
            // both allocations contain the full stride-by-height buffer.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (source + offset) as *const u8,
                    (destination + offset) as *mut u8,
                    row_bytes,
                );
            }
        }
    }

    fn clipped_region(&self, region: DisplayRegion) -> DisplayRegion {
        let x = region.x.min(self.config.width);
        let y = region.y.min(self.config.height);
        DisplayRegion::new(
            x,
            y,
            region.width.min(self.config.width.saturating_sub(x)),
            region.height.min(self.config.height.saturating_sub(y)),
        )
    }
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
        Ok(self.render.as_paddr())
    }

    fn present_framebuffer_region(
        &self,
        config: &FramebufferConfig,
        physical_addr: usize,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        if physical_addr != self.render.as_paddr()
            || config.width != self.config.width
            || config.height != self.config.height
            || config.stride != self.config.stride
            || config.format != self.config.format
        {
            return Err("apple-dcp: framebuffer does not match render surface");
        }

        let region = self.clipped_region(region);
        if region.width == 0 || region.height == 0 {
            return Ok(());
        }

        let mut state = self.state.lock();
        let next = state.front ^ 1;
        self.copy_region(
            self.render.as_paddr(),
            self.scanout[next].as_paddr(),
            region,
        );
        arch::io_wmb();
        let layer = make_layer(&self.config, self.scanout_dva[next]);
        state
            .iboot
            .swap(&layer, self.config.width, self.config.height)?;

        // iBoot has no DRM-like vblank completion event. Wait one scan period
        // before recycling the buffer that was front-most before this swap.
        time::udelay(state.frame_delay_us);

        // Keep both scanout buffers coherent. The next damage-only present can
        // therefore update either buffer without resurrecting older pixels.
        self.copy_region(
            self.scanout[next].as_paddr(),
            self.scanout[state.front].as_paddr(),
            region,
        );
        state.front = next;
        Ok(())
    }

    fn scanout_buffer_count(&self) -> usize {
        self.scanout.len()
    }

    fn get_scanout_buffer_info(
        &self,
        index: usize,
    ) -> Result<(FramebufferConfig, usize), &'static str> {
        let scanout = self
            .scanout
            .get(index)
            .ok_or("apple-dcp: invalid scanout buffer index")?;
        Ok((self.config.clone(), scanout.as_paddr()))
    }

    fn present_scanout_buffer(&self, index: usize) -> Result<(), &'static str> {
        if index >= self.scanout.len() {
            return Err("apple-dcp: invalid scanout buffer index");
        }
        let dva = *self
            .scanout_dva
            .get(index)
            .ok_or("apple-dcp: invalid scanout DVA index")?;

        // Direct scanout is exposed to userspace as DeviceBurstable memory, so
        // compositor writes reach the point of coherency without a cacheable
        // alias. Order those writes before publishing the buffer to DCP.
        arch::io_wmb();

        let mut state = self.state.lock();
        if index == state.front {
            return Err("apple-dcp: scanout buffer is already front-most");
        }
        let layer = make_layer(&self.config, dva);
        state
            .iboot
            .swap(&layer, self.config.width, self.config.height)?;

        state.front = index;
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

fn firmware_is_v13(device: &PlatformDeviceInfo) -> bool {
    let Some(value) = device.property("apple,firmware-compat") else {
        return true;
    };
    let value = value.value();
    let major_end = value
        .iter()
        .position(|byte| *byte == b'.')
        .unwrap_or(value.len());
    core::str::from_utf8(&value[..major_end])
        .ok()
        .and_then(|major| major.parse::<u32>().ok())
        .map(|major| major >= 13)
        .unwrap_or(true)
}

fn probe_deferred(message: &'static str) -> Result<(), &'static str> {
    early_println!("[apple-dcp] {}, deferring", message);
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

    let Some(dcp_dart) = get_dart_by_phandle(dcp_dart_phandle) else {
        return probe_deferred("DCP DART is not ready");
    };
    let Some(display_dart) = get_dart_by_phandle(display_dart_phandle) else {
        return probe_deferred("display DART is not ready");
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

    let dcp_handoff = map_handoff_regions(&mut dcp_table.lock(), dcp_phandle)?;
    let display_handoff = map_handoff_regions(&mut display_table.lock(), display_phandle)?;
    if dcp_handoff == 0 {
        return Err("apple-dcp: no firmware handoff mappings");
    }

    dcp_dart.enable_translation(dcp_stream, dcp_root, dcp_table.lock().translation_levels());
    display_dart.enable_translation(
        display_stream,
        display_root,
        display_table.lock().translation_levels(),
    );

    let mailbox_phandle = property_phandle(device, "mboxes")
        .or_else(|| property_phandle(device, "mailboxes"))
        .ok_or("apple-dcp: missing ASC mailbox")?;
    let Some(asc) = get_apple_asc_by_phandle(mailbox_phandle) else {
        return probe_deferred("ASC mailbox is not ready");
    };

    let page_size = 1usize << dcp_dart.page_shift();
    let mapper = Arc::new(DcpDmaMapper::new(
        Arc::clone(&dcp_table),
        Arc::clone(&dcp_dart),
        dcp_stream,
        page_size,
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
    let mut iboot = DcpIboot {
        endpoint,
        channel,
        firmware_v13: firmware_is_v13(device),
    };

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
    let frame_delay_us = if timing.fps == 0 {
        17_667
    } else {
        (1_000_000u64 << 16).div_ceil(timing.fps as u64) + 1_000
    };
    let visible_size = config.size();
    let allocation_size = visible_size
        .checked_add(24 * page_size)
        .ok_or("apple-dcp: scanout size overflow")?
        .div_ceil(page_size)
        * page_size;
    let render_pages = visible_size.div_ceil(environment::PAGE_SIZE);
    let scanout_pages = allocation_size.div_ceil(environment::PAGE_SIZE);
    let render = ContiguousPages::new(render_pages).ok_or("apple-dcp: render allocation failed")?;
    let scanout0 = ContiguousPages::new_aligned(scanout_pages, page_size)
        .ok_or("apple-dcp: scanout 0 allocation failed")?;
    let scanout1 = ContiguousPages::new_aligned(scanout_pages, page_size)
        .ok_or("apple-dcp: scanout 1 allocation failed")?;

    // SAFETY: all three page allocations are live and cover the requested sizes.
    unsafe {
        core::ptr::write_bytes(render.as_ptr() as *mut u8, 0, visible_size);
        core::ptr::write_bytes(scanout0.as_ptr() as *mut u8, 0, allocation_size);
        core::ptr::write_bytes(scanout1.as_ptr() as *mut u8, 0, allocation_size);
    }
    arch::clean_dcache_to_poc_range(scanout0.as_ptr() as usize, allocation_size);
    arch::clean_dcache_to_poc_range(scanout1.as_ptr() as usize, allocation_size);

    let scanout_dva = [
        DCP_SCANOUT_IOVA_BASE,
        DCP_SCANOUT_IOVA_BASE + allocation_size,
    ];
    for (index, scanout) in [&scanout0, &scanout1].iter().enumerate() {
        dcp_table.lock().map_contiguous(
            scanout_dva[index],
            scanout.as_paddr(),
            allocation_size,
            DCP_DART_FLAGS,
        )?;
        display_table.lock().map_contiguous(
            scanout_dva[index],
            scanout.as_paddr(),
            allocation_size,
            DCP_DART_FLAGS,
        )?;
    }
    let dcp_geometry = {
        let table = dcp_table.lock();
        (table.root_paddr(), table.translation_levels())
    };
    let display_geometry = {
        let table = display_table.lock();
        (table.root_paddr(), table.translation_levels())
    };
    dcp_dart.enable_translation(dcp_stream, dcp_geometry.0, dcp_geometry.1);
    display_dart.enable_translation(display_stream, display_geometry.0, display_geometry.1);

    arch::io_wmb();
    iboot.set_surface(&make_layer(&config, scanout_dva[0] as u64))?;

    let graphics = Arc::new(AppleDcpGraphics {
        config: config.clone(),
        render,
        scanout: [scanout0, scanout1],
        scanout_dva: [scanout_dva[0] as u64, scanout_dva[1] as u64],
        state: Mutex::new(DcpState {
            iboot,
            front: 0,
            frame_delay_us,
        }),
        _rtkit: rtkit,
        _dcp_table: dcp_table,
        _display_table: display_table,
    });
    let device_id = DeviceManager::get_manager()
        .register_device_with_name(alloc::string::String::from("apple-dcp"), graphics.clone());
    scarlet::device::graphics::manager::GraphicsManager::get_manager()
        .register_framebuffer_from_device(device_id, graphics)?;

    if scarlet::earlyfb::is_initialized() {
        scarlet::earlyfb::deactivate();
    }

    early_println!(
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
