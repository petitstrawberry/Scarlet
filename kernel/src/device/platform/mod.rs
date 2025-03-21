pub mod resource;

extern crate alloc;
use alloc::vec::Vec;

use super::*;
use resource::*;

pub struct PlatformDeviceInfo {
    name: &'static str,
    id: usize,
    compatible: &'static[&'static str],
}

impl PlatformDeviceInfo {
    pub fn new(name: &'static str, id: usize, compatible: &'static [&'static str]) -> Self {
        Self {
            name,
            id,
            compatible,
        }
    }
}

impl DeviceInfo for PlatformDeviceInfo {
    fn name(&self) -> &'static str {
        self.name
    }

    fn id(&self) -> usize {
        self.id
    }

    fn compatible(&self) -> &'static [&'static str] {
        self.compatible
    }
}

pub struct PlatformDeviceDriver {
    name: &'static str,
    resources: Vec<PlatformDeviceResource>,
    probe_fn: fn(&dyn DeviceInfo) -> Result<(), &'static str>,
    remove_fn: fn(&dyn DeviceInfo) -> Result<(), &'static str>,
    compatible: &'static [&'static str], 
}

impl PlatformDeviceDriver {
    pub fn new(
        name: &'static str,
        resources: Vec<PlatformDeviceResource>,
        probe_fn: fn(&dyn DeviceInfo) -> Result<(), &'static str>,
        remove_fn: fn(&dyn DeviceInfo) -> Result<(), &'static str>,
        compatible: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            resources,
            probe_fn,           
            remove_fn,
            compatible,
        }
    }
}

impl DeviceDriver for PlatformDeviceDriver {
    fn name(&self) -> &'static str {
        self.name
    }

    fn match_table(&self) -> &'static[&'static str] {
        self.compatible
    }

    fn probe(&self, device: &dyn DeviceInfo) -> Result<(), &'static str> {
        (self.probe_fn)(device)
    }

    fn remove(&self, _device: &dyn DeviceInfo) -> Result<(), &'static str> {
        Ok(())
    }
}

