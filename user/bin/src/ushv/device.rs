extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;

pub trait MmioDevice {
    fn base(&self) -> u64;
    fn size(&self) -> u64;
    fn read(&mut self, offset: u64, size: u8) -> u64;
    fn write(&mut self, offset: u64, size: u8, data: u64);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub struct FdtNodeInfo {
    pub name: String,
    pub compatible: String,
    pub reg: Vec<(u64, u64)>,
    pub interrupts: Vec<u32>,
    pub interrupt_parent: Option<u32>,
    pub extra: Vec<(String, FdtValue)>,
}

pub enum FdtValue {
    String(String),
    U32(u32),
    U64(u64),
    ArrayU32(Vec<u32>),
    ArrayU64(Vec<u64>),
    Empty,
}

pub trait DeviceFdt {
    fn fdt_node(&self) -> Option<FdtNodeInfo>;
}
