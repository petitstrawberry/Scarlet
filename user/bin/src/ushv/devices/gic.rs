extern crate alloc;

use alloc::string::String;
use alloc::vec;

use crate::device::{DeviceFdt, FdtNodeInfo, FdtValue, MmioDevice};

const GIC_PHANDLE: u32 = 1;

pub struct GicDevice {
    dist_base: u64,
    redist_base: u64,
}

impl GicDevice {
    pub fn new(dist_base: u64, redist_base: u64) -> Self {
        Self {
            dist_base,
            redist_base,
        }
    }
}

impl MmioDevice for GicDevice {
    fn base(&self) -> u64 {
        self.dist_base
    }

    fn size(&self) -> u64 {
        0x0100_0000
    }

    fn read(&self, _offset: u64, _size: u8) -> u64 {
        0
    }

    fn write(&self, _offset: u64, _size: u8, _data: u64) {}

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl DeviceFdt for GicDevice {
    fn fdt_node(&self) -> Option<FdtNodeInfo> {
        Some(FdtNodeInfo {
            name: alloc::format!("interrupt-controller@{:x}", self.dist_base),
            compatible: String::from("arm,gic-v3"),
            reg: vec![
                (self.dist_base, 0x0001_0000),
                (self.redist_base, 0x0002_0000),
            ],
            interrupts: vec![],
            interrupt_parent: None,
            extra: vec![
                (String::from("#interrupt-cells"), FdtValue::U32(3)),
                (String::from("interrupt-controller"), FdtValue::Empty),
                (String::from("phandle"), FdtValue::U32(GIC_PHANDLE)),
                (String::from("linux,phandle"), FdtValue::U32(GIC_PHANDLE)),
            ],
        })
    }
}
