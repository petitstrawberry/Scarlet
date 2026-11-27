//! Report-oriented HID bus. A transport attaches a discovered HID interface
//! after it can exchange reports and drops its HidDevice on removal.
//! All instance callbacks are serialized on the transport's worker.
use crate::sync::Mutex;
use alloc::{boxed::Box, sync::Arc, vec::Vec};

mod generic;
pub(super) mod raw;

/// Validate the HID item structure without requiring a known usage or driver.
pub fn valid_report_descriptor(bytes: &[u8]) -> bool {
    generic::valid_descriptor(bytes)
}

/// Attachment point. Bluetooth addresses are in HCI wire order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidTransport {
    Bluetooth {
        controller: [u8; 6],
        peer: [u8; 6],
    },
    Usb {
        bus: u16,
        address: u8,
        interface: u8,
    },
}

/// Product identity. Source 1 uses Bluetooth SIG numbers, source 2 uses USB-IF numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidIdentity {
    pub vendor_id_source: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version: u16,
}

#[derive(Clone, Debug)]
pub struct HidDeviceInfo {
    pub transport: HidTransport,
    pub identity: Option<HidIdentity>,
    pub name: Vec<u8>,
    pub report_descriptor: Vec<u8>,
    /// Maximum bytes in one output report, including any report ID.
    pub max_output_len: usize,
}

/// Signals a change to pending output or a driver deadline.
pub type HidWake = Arc<dyn Fn() + Send + Sync>;

/// A built-in or board-supplied driver registered before transport enumeration.
pub trait HidDriver: Send + Sync {
    fn matches(&self, info: &HidDeviceInfo) -> bool;
    fn probe(
        &self,
        info: &HidDeviceInfo,
        wake: HidWake,
    ) -> Result<Box<dyn HidInstance>, &'static str>;
}

/// One exclusive kernel binding. Drop releases the device's published state.
/// Reports preserve the descriptor's wire layout, including the ID for numbered reports.
pub trait HidInstance: Send {
    fn input_report(&mut self, report: &[u8]) -> Result<(), &'static str>;

    /// Return one complete output report. The transport retains it under backpressure.
    fn poll_output(&mut self, _now_ms: u64) -> Result<Option<Vec<u8>>, &'static str> {
        Ok(None)
    }

    /// Absolute monotonic time for the next poll. Zero requests an immediate poll.
    fn next_deadline_ms(&self) -> Option<u64> {
        None
    }

    /// Permit hidraw writes alongside this binding (e.g. an input-only driver).
    fn allow_raw_output(&self) -> bool {
        false
    }
}

static DRIVERS: Mutex<Vec<&'static dyn HidDriver>> = Mutex::new(Vec::new());

/// Register a resident driver from its initcall. The first successful matching
/// probe wins. The descriptor-driven input driver is the final fallback.
pub fn register_driver(driver: &'static dyn HidDriver) {
    DRIVERS.lock().push(driver);
}

fn bind(
    info: &HidDeviceInfo,
    wake: HidWake,
    drivers: &[&dyn HidDriver],
) -> Option<Box<dyn HidInstance>> {
    drivers
        .iter()
        .filter(|driver| driver.matches(info))
        .find_map(|driver| driver.probe(info, wake.clone()).ok())
        .or_else(|| generic::probe(info))
}

/// Transport-owned lifetime for one HID interface, its kernel binding, and hidraw node.
pub struct HidDevice {
    raw: raw::RawDevice,
    driver: Option<Box<dyn HidInstance>>,
    max_output_len: usize,
}

impl HidDevice {
    pub fn attach(info: HidDeviceInfo, wake: HidWake) -> Result<Self, &'static str> {
        if !valid_report_descriptor(&info.report_descriptor) {
            return Err("invalid HID report descriptor");
        }
        // Never call into drivers or the device manager under the registry lock.
        let drivers = DRIVERS.lock().clone();
        let driver = bind(&info, wake.clone(), &drivers);
        let allow_output = driver.as_ref().is_none_or(|d| d.allow_raw_output());
        let raw = raw::RawDevice::publish(&info, allow_output, wake)?;
        Ok(Self {
            raw,
            driver,
            max_output_len: info.max_output_len,
        })
    }

    pub fn set_name(&self, name: &[u8]) {
        self.raw.set_name(name);
    }

    pub fn input_report(&mut self, report: &[u8]) -> Result<(), &'static str> {
        self.raw.push_input(report);
        if let Some(driver) = &mut self.driver {
            driver.input_report(report)?;
        }
        Ok(())
    }

    pub fn poll_output(&mut self, now_ms: u64) -> Result<Option<Vec<u8>>, &'static str> {
        let from_driver = match &mut self.driver {
            Some(driver) => driver.poll_output(now_ms)?,
            None => None,
        };
        let report = from_driver.or_else(|| self.raw.take_output());
        if report
            .as_ref()
            .is_some_and(|p| p.is_empty() || p.len() > self.max_output_len)
        {
            return Err("invalid HID output report length");
        }
        Ok(report)
    }

    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.driver.as_ref()?.next_deadline_ms()
    }
}
