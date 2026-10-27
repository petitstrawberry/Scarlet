#![allow(dead_code)]
extern crate alloc;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub struct PciAddress {
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}
impl PciAddress {
    pub const fn new(segment: u16, bus: u8, device: u8, function: u8) -> Self {
        Self {
            segment,
            bus,
            device,
            function,
        }
    }
    pub const fn ecam_offset(&self) -> usize {
        ((self.bus as usize) << 20)
            | ((self.device as usize) << 15)
            | ((self.function as usize) << 12)
    }
}
mod arch {
    pub mod mmio {
        pub unsafe fn read8(a: usize) -> u8 {
            unsafe { core::ptr::read_volatile(a as *const u8) }
        }
        pub unsafe fn read16(a: usize) -> u16 {
            unsafe { core::ptr::read_volatile(a as *const u16) }
        }
        pub unsafe fn read32(a: usize) -> u32 {
            unsafe { core::ptr::read_volatile(a as *const u32) }
        }
        pub unsafe fn write8(a: usize, v: u8) {
            unsafe {
                core::ptr::write_volatile(a as *mut u8, v);
            }
        }
        pub unsafe fn write16(a: usize, v: u16) {
            unsafe {
                core::ptr::write_volatile(a as *mut u16, v);
            }
        }
        pub unsafe fn write32(a: usize, v: u32) {
            unsafe {
                core::ptr::write_volatile(a as *mut u32, v);
            }
        }
    }
}
mod config;

#[derive(Default)]
struct WindowHost {
    calls: Mutex<Vec<(bool, usize, usize, u32)>>,
}
impl config::PciConfigAccess for WindowHost {
    fn is_valid_address(&self, a: &PciAddress) -> bool {
        a.segment == 7 && a.bus == 1 && a.device == 0 && a.function == 0
    }
    fn read(&self, _: &PciAddress, o: usize, w: usize) -> u32 {
        self.calls.lock().unwrap().push((false, o, w, 0));
        0x43ec14e4
    }
    fn write(&self, _: &PciAddress, o: usize, w: usize, v: u32) {
        self.calls.lock().unwrap().push((true, o, w, v));
    }
}

#[test]
fn non_ecam_config_and_cloned_endpoint_use_the_host_protocol() {
    let host = Arc::new(WindowHost::default());
    let cfg = config::PciConfig::from_host(host.clone());
    let endpoint = cfg.clone();
    let addr = PciAddress::new(7, 1, 0, 0);
    assert_eq!(endpoint.read_u32(&addr, 0), 0x43ec14e4);
    endpoint.write_u16(&addr, 4, 6);
    endpoint.write_u8(&addr, 0x3c, 0xff);
    assert_eq!(
        *host.calls.lock().unwrap(),
        vec![(false, 0, 4, 0), (true, 4, 2, 6), (true, 0x3c, 1, 0xff)]
    );
    drop(cfg);
    assert_eq!(endpoint.read_vendor_id(&addr), 0x14e4);
}

#[test]
fn invalid_offsets_widths_functions_and_segments_never_reach_the_host() {
    let host = Arc::new(WindowHost::default());
    let cfg = config::PciConfig::from_host(host.clone());
    let addr = PciAddress::new(7, 1, 0, 0);
    assert_eq!(cfg.read_u32(&addr, 4096), u32::MAX);
    assert_eq!(cfg.read_u32(&addr, usize::MAX), u32::MAX);
    assert_eq!(cfg.read_u16(&addr, 3), u16::MAX);
    assert_eq!(cfg.read_u8(&PciAddress::new(0, 1, 0, 0), 0), u8::MAX);
    cfg.write_u32(&addr, 2, 0);
    cfg.write_u8(&PciAddress::new(7, 1, 32, 0), 0, 0);
    assert!(host.calls.lock().unwrap().is_empty());
}

#[test]
fn bounded_ecam_command_write_does_not_rewrite_adjacent_status() {
    let mut words = [0u32; 1024];
    words[1] = 0xf900_0000;
    let cfg = config::PciConfig::bounded_ecam(words.as_mut_ptr() as usize, 4096);
    let addr = PciAddress::new(0, 0, 0, 0);
    cfg.write_u16(&addr, 4, 6);
    cfg.write_u8(&addr, 0x3c, 0x80);
    assert_eq!(words[1], 0xf900_0006);
    assert_eq!(cfg.read_u8(&addr, 0x3c), 0x80);
    assert_eq!(cfg.read_u32(&PciAddress::new(0, 0, 1, 0), 0), u32::MAX);
}
