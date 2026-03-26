#![no_std]
extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use scarlet::sync::Mutex;

use scarlet::device::graphics::FramebufferConfig;
use scarlet::device::graphics::output::DisplayOutput;
use scarlet::device::manager::{DeviceManager, DriverPriority};
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::drivers::soc::apple_afk::AfkEndpoint;
use scarlet::drivers::soc::apple_asc::AppleAsc;
use scarlet::drivers::soc::apple_epic::EpicEndpoint;
use scarlet::drivers::soc::apple_rtkit::AppleRtkit;
use scarlet::early_println;
use scarlet::vm;
use scarlet_driver_apple_dart::{DartPageTable, get_dart_by_phandle};

const DCP_SYSTEM_EP: u8 = 0x20;
const DCP_DPTX_PORT_EP: u8 = 0x2a;
const DCP_IBOOT_EP: u8 = 0x23;

const DCP_ASC_OFFSET: usize = 0x8000;
const DCP_SERVICE_WAIT_TIMEOUT_US: u64 = 5_000_000;
const DCP_DVA_OFFSET: u64 = 0xf000_0000;
const DCP_DART_FB_FLAGS: u64 = 1;

const SYSTEM_SERVICE_PREFIX: &str = "system";
const DPTX_SERVICE_PREFIX: &str = "AppleDCPDPTXRemotePort";

const DPTX_GROUP: u16 = 0;
const DPTX_APCALL_GET_LINK_RATE: u32 = 8;
const DPTX_APCALL_SET_LINK_RATE: u32 = 9;
const DPTX_APCALL_GET_ACTIVE_LANE_COUNT: u32 = 11;
const DPTX_APCALL_SET_ACTIVE_LANE_COUNT: u32 = 12;
const DPTX_APCALL_GET_HPD_STATUS: u32 = 13;
const DPTX_APCALL_FORCE_HOTPLUG_DETECT: u32 = 19;

const IBOOT_GROUP: u16 = 0;
const IBOOT_CALL_SWAP_BEGIN: u32 = 4;
const IBOOT_CALL_SWAP_SET_LAYER: u32 = 5;
const IBOOT_CALL_SWAP_END: u32 = 6;

pub struct DcpextOutput {
    dart_phandle: u32,
    iova_base: usize,
    page_table: Mutex<Option<DartPageTable>>,
    connected: AtomicBool,
}

impl DcpextOutput {
    pub fn new(dart_phandle: u32) -> Self {
        Self {
            dart_phandle,
            iova_base: 0x1000_0000,
            page_table: Mutex::new(None),
            connected: AtomicBool::new(false),
        }
    }
}

impl DisplayOutput for DcpextOutput {
    fn name(&self) -> &str {
        "dp0"
    }

    fn is_connected(&self) -> bool {
        if let Some(connected) = with_dcpext(|dcp| dcp.hotplug_detect().unwrap_or(false)) {
            self.connected.store(connected, Ordering::Relaxed);
            return connected;
        }
        self.connected.load(Ordering::Relaxed)
    }

    fn present(&self, config: &FramebufferConfig, fb_paddr: usize) -> Result<(), &'static str> {
        {
            let mut pt_guard = self.page_table.lock();
            if pt_guard.is_none() {
                let pt = DartPageTable::new()?;
                let dart = get_dart_by_phandle(self.dart_phandle)
                    .ok_or("apple-dcpext-output: DART not found")?;
                dart.enable_translation(0, pt.root_paddr(), 2);
                *pt_guard = Some(pt);
            }

            let pt = pt_guard
                .as_mut()
                .ok_or("apple-dcpext-output: DART page table unavailable")?;
            pt.map_contiguous(self.iova_base, fb_paddr, config.size(), DCP_DART_FB_FLAGS)?;
        }

        let fb_iova = DCP_DVA_OFFSET | self.iova_base as u64;

        with_dcpext(|dcp| {
            dcp.mirror_framebuffer(
                fb_paddr as u64,
                config.width,
                config.height,
                config.stride,
                fb_iova,
            )
        })
        .ok_or("apple-dcpext-output: DCPext not initialized")?
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct IBootPlaneInfo {
    unk1: u32,
    addr: u64,
    tile_size: u32,
    stride: u32,
    unk5: u32,
    unk6: u32,
    unk7: u32,
    unk8: u32,
    addr_format: u32,
    unk9: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct IBootLayerInfo {
    planes: [IBootPlaneInfo; 3],
    unk: u32,
    plane_cnt: u32,
    width: u32,
    height: u32,
    surface_fmt: u32,
    colorspace: u32,
    eotf: u32,
    transform: u32,
    _pad: [u8; 3],
}

pub struct AppleDcpExt {
    coproc_base: usize,
    _coproc_size: usize,
    _asc: Option<Arc<AppleAsc>>,
    _rtkit: Option<Arc<AppleRtkit>>,
    system_ep: Option<EpicEndpoint>,
    dptx_port_ep: Option<EpicEndpoint>,
    iboot_ep: Option<EpicEndpoint>,
    initialized: bool,
}

impl AppleDcpExt {
    fn new(coproc_base: usize, coproc_size: usize) -> Self {
        Self {
            coproc_base,
            _coproc_size: coproc_size,
            _asc: None,
            _rtkit: None,
            system_ep: None,
            dptx_port_ep: None,
            iboot_ep: None,
            initialized: false,
        }
    }

    fn asc_base(&self) -> usize {
        self.coproc_base + DCP_ASC_OFFSET
    }

    fn create_rtkit(&self) -> (Arc<AppleAsc>, Arc<AppleRtkit>) {
        let asc = Arc::new(AppleAsc::new(self.asc_base()));
        let rtkit = Arc::new(AppleRtkit::new(Arc::clone(&asc)));
        (asc, rtkit)
    }

    fn create_epic_endpoint(
        rtkit: Arc<AppleRtkit>,
        endpoint: u8,
    ) -> Result<EpicEndpoint, &'static str> {
        let afk = Arc::new(Mutex::new(AfkEndpoint::new(rtkit, endpoint)));
        afk.lock().start()?;
        EpicEndpoint::new(afk)
    }

    fn wait_for_named_service(
        epic: &mut EpicEndpoint,
        name_prefix: &str,
    ) -> Result<(), &'static str> {
        if epic.find_service(name_prefix).is_some() {
            return Ok(());
        }

        epic.wait_for_services(1, DCP_SERVICE_WAIT_TIMEOUT_US)?;

        if epic.find_service(name_prefix).is_some() {
            return Ok(());
        }

        Err("apple-dcpext: expected service not announced")
    }

    fn ensure_ready(&self) -> Result<(), &'static str> {
        if self.initialized {
            Ok(())
        } else {
            Err("apple-dcpext: endpoint stack not initialized")
        }
    }

    fn init(&mut self) -> Result<(), &'static str> {
        early_println!(
            "[apple-dcpext] init: coproc_base={:#x} asc_base={:#x}",
            self.coproc_base,
            self.asc_base()
        );

        let (asc, rtkit) = self.create_rtkit();
        rtkit.boot()?;

        let mut system_ep = Self::create_epic_endpoint(Arc::clone(&rtkit), DCP_SYSTEM_EP)?;
        let mut dptx_port_ep = Self::create_epic_endpoint(Arc::clone(&rtkit), DCP_DPTX_PORT_EP)?;
        let mut iboot_ep = Self::create_epic_endpoint(Arc::clone(&rtkit), DCP_IBOOT_EP)?;

        Self::wait_for_named_service(&mut system_ep, SYSTEM_SERVICE_PREFIX)
            .map_err(|_| "apple-dcpext: system service not announced")?;
        Self::wait_for_named_service(&mut dptx_port_ep, DPTX_SERVICE_PREFIX)
            .map_err(|_| "apple-dcpext: DPTX remote port service not announced")?;
        Self::wait_for_named_service(&mut iboot_ep, "disp0")
            .map_err(|_| "apple-dcpext: disp0-service not announced")?;

        early_println!("[apple-dcpext] init complete");

        self._asc = Some(asc);
        self._rtkit = Some(rtkit);
        self.system_ep = Some(system_ep);
        self.dptx_port_ep = Some(dptx_port_ep);
        self.iboot_ep = Some(iboot_ep);
        self.initialized = true;

        Ok(())
    }

    fn dptx_service_channel(ep: &EpicEndpoint) -> Result<u32, &'static str> {
        ep.find_service(DPTX_SERVICE_PREFIX)
            .map(|service| service.channel)
            .ok_or("apple-dcpext: DPTX service not found")
    }

    fn dptx_call(&mut self, command: u32, data: &[u8]) -> Result<Vec<u8>, &'static str> {
        self.ensure_ready()?;

        let ep = self
            .dptx_port_ep
            .as_mut()
            .ok_or("apple-dcpext: DPTX EPIC endpoint not initialized")?;
        let channel = Self::dptx_service_channel(ep)?;
        ep.call_by_channel(channel, DPTX_GROUP, command, data)
    }

    fn iboot_call(&mut self, command: u32, data: &[u8]) -> Result<Vec<u8>, &'static str> {
        self.ensure_ready()?;

        let ep = self
            .iboot_ep
            .as_mut()
            .ok_or("apple-dcpext: IBoot EPIC endpoint not initialized")?;
        let channel = ep
            .find_service("disp0")
            .map(|service| service.channel)
            .ok_or("apple-dcpext: disp0 service not found")?;
        ep.call_by_channel(channel, IBOOT_GROUP, command, data)
    }

    fn read_u32_reply(reply: &[u8]) -> Result<u32, &'static str> {
        if reply.len() < 4 {
            return Err("apple-dcpext: short EPIC reply");
        }

        Ok(u32::from_le_bytes([reply[0], reply[1], reply[2], reply[3]]))
    }

    fn write_u32_payload(value: u32) -> [u8; 4] {
        value.to_le_bytes()
    }

    fn call_get_u32(&mut self, command: u32) -> Result<u32, &'static str> {
        let reply = self.dptx_call(command, &[])?;
        Self::read_u32_reply(&reply)
    }

    fn call_set_u32(&mut self, command: u32, value: u32) -> Result<(), &'static str> {
        let payload = Self::write_u32_payload(value);
        let _ = self.dptx_call(command, &payload)?;
        Ok(())
    }

    pub fn hotplug_detect(&mut self) -> Result<bool, &'static str> {
        let _ = self.dptx_call(DPTX_APCALL_FORCE_HOTPLUG_DETECT, &[])?;
        Ok(self.call_get_u32(DPTX_APCALL_GET_HPD_STATUS)? != 0)
    }

    pub fn get_link_rate(&mut self) -> Result<u32, &'static str> {
        self.call_get_u32(DPTX_APCALL_GET_LINK_RATE)
    }

    pub fn set_link_rate(&mut self, rate: u32) -> Result<(), &'static str> {
        self.call_set_u32(DPTX_APCALL_SET_LINK_RATE, rate)
    }

    pub fn get_lane_count(&mut self) -> Result<u32, &'static str> {
        self.call_get_u32(DPTX_APCALL_GET_ACTIVE_LANE_COUNT)
    }

    pub fn set_lane_count(&mut self, lanes: u32) -> Result<(), &'static str> {
        self.call_set_u32(DPTX_APCALL_SET_ACTIVE_LANE_COUNT, lanes)
    }

    fn create_layer_info(
        fb_iova: u64,
        width: u32,
        height: u32,
        stride: u32,
        surface_fmt: u32,
    ) -> IBootLayerInfo {
        IBootLayerInfo {
            planes: [
                IBootPlaneInfo {
                    unk1: 0,
                    addr: fb_iova,
                    tile_size: 0,
                    stride,
                    unk5: 0,
                    unk6: 0,
                    unk7: 0,
                    unk8: 0,
                    addr_format: 1,
                    unk9: 0,
                },
                IBootPlaneInfo {
                    unk1: 0,
                    addr: 0,
                    tile_size: 0,
                    stride: 0,
                    unk5: 0,
                    unk6: 0,
                    unk7: 0,
                    unk8: 0,
                    addr_format: 0,
                    unk9: 0,
                },
                IBootPlaneInfo {
                    unk1: 0,
                    addr: 0,
                    tile_size: 0,
                    stride: 0,
                    unk5: 0,
                    unk6: 0,
                    unk7: 0,
                    unk8: 0,
                    addr_format: 0,
                    unk9: 0,
                },
            ],
            unk: 0,
            plane_cnt: 1,
            width,
            height,
            surface_fmt,
            colorspace: 1,
            eotf: 1,
            transform: 0,
            _pad: [0; 3],
        }
    }

    pub fn swap_begin(&mut self) -> Result<(), &'static str> {
        self.iboot_call(IBOOT_CALL_SWAP_BEGIN, &[])?;
        Ok(())
    }

    pub fn swap_set_layer(
        &mut self,
        layer_index: u32,
        layer: &IBootLayerInfo,
        src_rect: (u32, u32, u32, u32),
        dst_rect: (u32, u32, u32, u32),
    ) -> Result<(), &'static str> {
        let layer_size = core::mem::size_of::<IBootLayerInfo>();
        let mut payload = alloc::vec![0u8; 8 + layer_size + 32];

        payload[0..4].copy_from_slice(&layer_index.to_le_bytes());

        let layer_bytes =
            unsafe { core::slice::from_raw_parts(layer as *const _ as *const u8, layer_size) };
        payload[8..8 + layer_bytes.len()].copy_from_slice(layer_bytes);

        let src_offset = 8 + layer_bytes.len();
        payload[src_offset..src_offset + 4].copy_from_slice(&src_rect.0.to_le_bytes());
        payload[src_offset + 4..src_offset + 8].copy_from_slice(&src_rect.1.to_le_bytes());
        payload[src_offset + 8..src_offset + 12].copy_from_slice(&src_rect.2.to_le_bytes());
        payload[src_offset + 12..src_offset + 16].copy_from_slice(&src_rect.3.to_le_bytes());

        let dst_offset = src_offset + 16;
        payload[dst_offset..dst_offset + 4].copy_from_slice(&dst_rect.0.to_le_bytes());
        payload[dst_offset + 4..dst_offset + 8].copy_from_slice(&dst_rect.1.to_le_bytes());
        payload[dst_offset + 8..dst_offset + 12].copy_from_slice(&dst_rect.2.to_le_bytes());
        payload[dst_offset + 12..dst_offset + 16].copy_from_slice(&dst_rect.3.to_le_bytes());

        self.iboot_call(IBOOT_CALL_SWAP_SET_LAYER, &payload)?;
        Ok(())
    }

    pub fn swap_end(&mut self) -> Result<(), &'static str> {
        self.iboot_call(IBOOT_CALL_SWAP_END, &[])?;
        Ok(())
    }

    pub fn mirror_framebuffer(
        &mut self,
        fb_paddr: u64,
        fb_width: u32,
        fb_height: u32,
        fb_stride: u32,
        fb_iova: u64,
    ) -> Result<(), &'static str> {
        let layer = Self::create_layer_info(fb_iova, fb_width, fb_height, fb_stride, 1);

        early_println!(
            "[apple-dcpext] mirror: {}x{} stride={} paddr={:#x} iova={:#x}",
            fb_width,
            fb_height,
            fb_stride,
            fb_paddr,
            fb_iova
        );

        self.swap_begin()?;
        self.swap_set_layer(
            0,
            &layer,
            (0, 0, fb_width, fb_height),
            (0, 0, fb_width, fb_height),
        )?;
        self.swap_end()?;

        early_println!("[apple-dcpext] mirror complete");
        Ok(())
    }

    pub fn poll(&mut self) {
        if self.ensure_ready().is_err() {
            return;
        }

        if let Some(ep) = self.system_ep.as_mut() {
            ep.poll();
        }
        if let Some(ep) = self.dptx_port_ep.as_mut() {
            ep.poll();
        }
        if let Some(ep) = self.iboot_ep.as_mut() {
            ep.poll();
        }
    }
}

static DCP_EXT: Mutex<Option<AppleDcpExt>> = Mutex::new(None);
static DCP_DART_PHANDLE: Mutex<Option<u32>> = Mutex::new(None);

pub fn with_dcpext<R>(f: impl FnOnce(&mut AppleDcpExt) -> R) -> Option<R> {
    let mut guard = DCP_EXT.lock();
    guard.as_mut().map(f)
}

pub fn mirror_boot_fb() -> Result<(), &'static str> {
    use scarlet::device::graphics::manager::GraphicsManager;

    let fb = GraphicsManager::get_manager()
        .get_framebuffer("fb0")
        .ok_or("apple-dcpext: fb0 not found")?;

    let dart_phandle = DCP_DART_PHANDLE
        .lock()
        .ok_or("apple-dcpext: dart phandle not set (probe may not have run)")?;

    let output = DcpextOutput::new(dart_phandle);
    output.present(&fb.config, fb.physical_addr)?;

    early_println!(
        "[apple-dcpext] mirrored fb0 {}x{} to dp0",
        fb.config.width,
        fb.config.height
    );

    Ok(())
}

fn has_mboxes_property(device: &PlatformDeviceInfo) -> bool {
    device
        .property("mboxes")
        .map(|property| !property.value().is_empty())
        .unwrap_or(false)
}

fn coproc_resource(device: &PlatformDeviceInfo) -> Result<(usize, usize), &'static str> {
    let mem_resources: Vec<_> = device
        .get_resources()
        .iter()
        .filter(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let coproc = mem_resources
        .first()
        .ok_or("apple-dcpext: missing coproc memory resource")?;

    let paddr = coproc.start;
    let size = coproc
        .end
        .checked_sub(coproc.start)
        .and_then(|value| value.checked_add(1))
        .ok_or("apple-dcpext: invalid coproc memory resource")?;

    Ok((paddr, size))
}

fn parse_iommus_phandle(device: &PlatformDeviceInfo) -> Option<u32> {
    let prop = device.property("iommus")?;
    let bytes = prop.value();
    if bytes.len() < 8 {
        return None;
    }
    let phandle = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    Some(phandle)
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let (coproc_paddr, coproc_size) = coproc_resource(device)?;

    let coproc_base = vm::ioremap(coproc_paddr, coproc_size)
        .map_err(|_| "apple-dcpext: failed to map coproc MMIO")?;

    if !has_mboxes_property(device) {
        early_println!("[apple-dcpext] warning: missing mboxes property");
    }

    early_println!(
        "[apple-dcpext] probe: coproc paddr={:#x} size={:#x}",
        coproc_paddr,
        coproc_size
    );

    let mut dcp = AppleDcpExt::new(coproc_base, coproc_size);
    dcp.init()?;

    *DCP_EXT.lock() = Some(dcp);

    if let Some(phandle) = parse_iommus_phandle(device) {
        early_println!("[apple-dcpext] dart phandle: {:#x}", phandle);
        *DCP_DART_PHANDLE.lock() = Some(phandle);
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    *DCP_EXT.lock() = None;
    Ok(())
}

fn register_dcpext_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-dcpext",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-dcpext", "apple,dcpext"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

scarlet::driver_initcall!(register_dcpext_driver);

#[used]
static SCARLET_DRIVER_DCPEXT_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
