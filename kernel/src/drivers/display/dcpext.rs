#![cfg(target_arch = "aarch64")]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use spin::Mutex;

use crate::device::manager::{DeviceManager, DriverPriority};
use crate::device::platform::resource::PlatformDeviceResourceType;
use crate::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use crate::driver_initcall;
use crate::drivers::soc::apple_afk::AfkEndpoint;
use crate::drivers::soc::apple_asc::AppleAsc;
use crate::drivers::soc::apple_epic::EpicEndpoint;
use crate::drivers::soc::apple_rtkit::AppleRtkit;
use crate::early_println;
use crate::vm;

const DCP_SYSTEM_EP: u8 = 0x20;
const DCP_DPTX_PORT_EP: u8 = 0x2a;

const DCP_ASC_OFFSET: usize = 0x8000;
const DCP_SERVICE_WAIT_TIMEOUT_US: u64 = 5_000_000;

const SYSTEM_SERVICE_PREFIX: &str = "system";
const DPTX_SERVICE_PREFIX: &str = "AppleDCPDPTXRemotePort";

const DPTX_GROUP: u16 = 0;
const DPTX_APCALL_GET_LINK_RATE: u32 = 8;
const DPTX_APCALL_SET_LINK_RATE: u32 = 9;
const DPTX_APCALL_GET_ACTIVE_LANE_COUNT: u32 = 11;
const DPTX_APCALL_SET_ACTIVE_LANE_COUNT: u32 = 12;
const DPTX_APCALL_GET_HPD_STATUS: u32 = 13;
const DPTX_APCALL_FORCE_HOTPLUG_DETECT: u32 = 19;

pub struct AppleDcpExt {
    coproc_base: usize,
    _coproc_size: usize,
    _asc: Option<Arc<AppleAsc>>,
    _rtkit: Option<Arc<AppleRtkit>>,
    system_ep: Option<EpicEndpoint>,
    dptx_port_ep: Option<EpicEndpoint>,
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

        Self::wait_for_named_service(&mut system_ep, SYSTEM_SERVICE_PREFIX)
            .map_err(|_| "apple-dcpext: system service not announced")?;
        Self::wait_for_named_service(&mut dptx_port_ep, DPTX_SERVICE_PREFIX)
            .map_err(|_| "apple-dcpext: DPTX remote port service not announced")?;

        early_println!("[apple-dcpext] init complete");

        self._asc = Some(asc);
        self._rtkit = Some(rtkit);
        self.system_ep = Some(system_ep);
        self.dptx_port_ep = Some(dptx_port_ep);
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

    pub fn poll(&mut self) {
        if self.ensure_ready().is_err() {
            return;
        }

        if let Some(system_ep) = self.system_ep.as_mut() {
            system_ep.poll();
        }

        if let Some(dptx_port_ep) = self.dptx_port_ep.as_mut() {
            dptx_port_ep.poll();
        }
    }
}

static DCP_EXT: Mutex<Option<AppleDcpExt>> = Mutex::new(None);

pub fn with_dcpext<R>(f: impl FnOnce(&mut AppleDcpExt) -> R) -> Option<R> {
    let mut guard = DCP_EXT.lock();
    guard.as_mut().map(f)
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

driver_initcall!(register_dcpext_driver);
