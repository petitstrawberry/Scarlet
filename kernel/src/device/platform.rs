use super::*;

pub struct PlatformDevice {
    name: &'static str,
    id: usize,
}

impl Device for PlatformDevice {
    fn name(&self) -> &'static str {
        self.name
    }

    fn id(&self) -> usize {
        self.id
    }
}

pub struct PlatformDeviceDriver {
    name: &'static str,
    match_fn: fn(&dyn Device) -> bool,
    probe_fn: fn(&dyn Device) -> Result<(), &'static str>,
}

impl PlatformDeviceDriver {
    pub fn new(
        name: &'static str,
        match_fn: fn(&dyn Device) -> bool,
        probe_fn: fn(&dyn Device) -> Result<(), &'static str>,
    ) -> Self {
        Self {
            name,
            match_fn,
            probe_fn,
        }
    }
}

impl DeviceDriver for PlatformDeviceDriver {
    fn name(&self) -> &'static str {
        self.name
    }

    fn match_device(&self, device: &dyn Device) -> bool {
        (self.match_fn)(device)
    }

    fn probe(&self, device: &dyn Device) -> Result<(), &'static str> {
        (self.probe_fn)(device)
    }
}

