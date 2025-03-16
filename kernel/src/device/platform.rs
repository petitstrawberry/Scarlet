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


#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    #[allow(static_mut_refs)]
    fn test_driver_register() {
        let len = unsafe { DRIVER_TABLE.len() };
        let driver = Box::new(PlatformDeviceDriver {
            name: "test",
            match_fn: |_device| _device.name() == "test",
            probe_fn: |_device| Ok(()),
        });
        driver_register(driver);
        assert_eq!(unsafe { DRIVER_TABLE.len() }, len + 1);
    }
}