extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;

pub trait MmioDevice: Send + Sync {
    fn base(&self) -> u64;
    fn size(&self) -> u64;
    fn read(&self, offset: u64, size: u8) -> u64;
    fn write(&self, offset: u64, size: u8, data: u64);
    fn as_any(&self) -> &dyn Any;
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

pub trait IrqSink: Send + Sync {
    fn set_level(&self, level: bool);
}

#[derive(Clone)]
pub struct IrqLine {
    sink: alloc::sync::Arc<dyn IrqSink>,
}

impl IrqLine {
    pub fn new(sink: alloc::sync::Arc<dyn IrqSink>) -> Self {
        Self { sink }
    }

    pub fn set(&self, level: bool) {
        self.sink.set_level(level);
    }
}
