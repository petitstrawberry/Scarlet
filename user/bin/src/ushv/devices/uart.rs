extern crate alloc;

use alloc::string::String;
use alloc::vec;
use scarlet_std::sync::RwLock;

use crate::device::{DeviceFdt, FdtNodeInfo, FdtValue, MmioDevice};
use scarlet_std::print;

const THR: u64 = 0x00;
const RBR: u64 = 0x00;
const IER: u64 = 0x01;
const FCR: u64 = 0x02;
const IIR: u64 = 0x02;
const LCR: u64 = 0x03;
const MCR: u64 = 0x04;
const LSR: u64 = 0x05;
const MSR: u64 = 0x06;
const SCR: u64 = 0x07;

const LSR_TX_EMPTY: u8 = 0x20;
const LSR_RX_READY: u8 = 0x01;

struct UartInner {
    base: u64,
    irq: u32,
    lcr: u8,
    lsr: u8,
    scr: u8,
}

impl UartInner {
    fn new(base: u64, irq: u32) -> Self {
        Self {
            base,
            irq,
            lcr: 0,
            lsr: LSR_TX_EMPTY,
            scr: 0,
        }
    }
}

pub struct Ns16550a {
    inner: RwLock<UartInner>,
}

impl Ns16550a {
    pub fn new(base: u64) -> Self {
        Self {
            inner: RwLock::new(UartInner::new(base, 10)),
        }
    }

    pub fn with_irq(base: u64, irq: u32) -> Self {
        Self {
            inner: RwLock::new(UartInner::new(base, irq)),
        }
    }
}

impl MmioDevice for Ns16550a {
    fn base(&self) -> u64 {
        self.inner.read().base
    }

    fn size(&self) -> u64 {
        0x1000
    }

    fn read(&mut self, offset: u64, _size: u8) -> u64 {
        let mut inner = self.inner.write();
        match offset {
            RBR => {
                inner.lsr &= !LSR_RX_READY;
                0
            }
            IER => 0,
            IIR => 0x01,
            LCR => inner.lcr as u64,
            MCR => 0,
            LSR => inner.lsr as u64,
            MSR => 0,
            SCR => inner.scr as u64,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, _size: u8, data: u64) {
        let byte = data as u8;
        let mut inner = self.inner.write();

        match offset {
            THR => {
                print!("{}", byte as char);
            }
            IER => {}
            FCR => {}
            LCR => {
                inner.lcr = byte;
            }
            MCR => {}
            SCR => {
                inner.scr = byte;
            }
            _ => {}
        }
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

impl DeviceFdt for Ns16550a {
    fn fdt_node(&self) -> Option<FdtNodeInfo> {
        let inner = self.inner.read();
        Some(FdtNodeInfo {
            name: alloc::format!("serial@{:x}", inner.base),
            compatible: String::from("ns16550a"),
            reg: vec![(inner.base, 0x100)],
            interrupts: vec![inner.irq],
            interrupt_parent: None,
            extra: vec![(String::from("clock-frequency"), FdtValue::U32(3686400))],
        })
    }
}
