#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
    device::{
        DeviceInfo,
        clk::ClkHandle,
        dma::DmaChannel,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
};

const APPLE_MCA_T8103_CLUSTERS: usize = 6;
const APPLE_MCA_T6000_CLUSTERS: usize = 4;

static APPLE_MCA_DEVICES: Mutex<Vec<Arc<AppleMca>>> = Mutex::new(Vec::new());

struct AppleMcaDma {
    name: String,
    channel: Arc<dyn DmaChannel>,
}

struct AppleMca {
    base: usize,
    switch_base: usize,
    size: usize,
    switch_size: usize,
    clocks: Vec<ClkHandle>,
    dmas: Vec<AppleMcaDma>,
}

impl AppleMca {
    fn new(
        base: usize,
        size: usize,
        switch_base: usize,
        switch_size: usize,
        clocks: Vec<ClkHandle>,
        dmas: Vec<AppleMcaDma>,
    ) -> Self {
        Self {
            base,
            switch_base,
            size,
            switch_size,
            clocks,
            dmas,
        }
    }
}

fn cluster_count(device: &PlatformDeviceInfo) -> usize {
    let compatible = device.compatible();
    if compatible
        .iter()
        .any(|entry| *entry == "apple,t6000-mca" || *entry == "apple,t6020-mca")
    {
        APPLE_MCA_T6000_CLUSTERS
    } else {
        APPLE_MCA_T8103_CLUSTERS
    }
}

fn resolve_clocks(
    device: &PlatformDeviceInfo,
    count: usize,
) -> Result<Vec<ClkHandle>, &'static str> {
    let manager = DeviceManager::get_manager();
    let mut clocks: Vec<ClkHandle> = Vec::new();

    for index in 0..count {
        let clk = manager.resolve_clk_by_index(device, index)?;
        if clk.prepare_enable().is_err() {
            for clock in &clocks {
                clock.disable_unprepare();
            }
            return Err("apple-mca: failed to enable clock");
        }
        clocks.push(clk);
    }

    Ok(clocks)
}

fn resolve_dmas(device: &PlatformDeviceInfo) -> Result<Vec<AppleMcaDma>, &'static str> {
    let names = device
        .property("dma-names")
        .ok_or("apple-mca: missing dma-names")?
        .as_string_list()
        .ok_or("apple-mca: malformed dma-names")?;
    let manager = DeviceManager::get_manager();
    let mut dmas = Vec::new();

    for name in names {
        let channel = manager.resolve_dma_channel(device, name)?;
        dmas.push(AppleMcaDma {
            name: name.to_string(),
            channel,
        });
    }

    Ok(dmas)
}

fn map_resource(device: &PlatformDeviceInfo, index: usize) -> Result<(usize, usize), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .filter(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .nth(index)
        .ok_or("apple-mca: missing memory resource")?;
    let paddr = resource.start;
    let size = resource.end - resource.start + 1;
    let base = scarlet::vm::ioremap(paddr, size).map_err(|_| "apple-mca: ioremap failed")?;

    Ok((base, size))
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let (base, size) = map_resource(device, 0)?;
    let (switch_base, switch_size) = map_resource(device, 1)?;
    let clusters = cluster_count(device);
    let clocks = resolve_clocks(device, clusters)?;
    let dmas = resolve_dmas(device)?;
    let dma_count = dmas.len();
    let mca = Arc::new(AppleMca::new(
        base,
        size,
        switch_base,
        switch_size,
        clocks,
        dmas,
    ));
    APPLE_MCA_DEVICES.lock().push(mca);

    early_println!(
        "[apple-mca] probed {} at base={:#x}, switch={:#x}, clusters={}, dmas={}",
        device.name(),
        base,
        switch_base,
        clusters,
        dma_count
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-mca",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-mca",
            "apple,t8112-mca",
            "apple,t6000-mca",
            "apple,t6020-mca",
            "apple,mca",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_MCA_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
