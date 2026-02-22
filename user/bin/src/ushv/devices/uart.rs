extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use scarlet_std::{println, sync::RwLock};

use crate::device::{DeviceFdt, FdtNodeInfo, FdtValue, IrqLine, MmioDevice};
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
    irq_out: Option<IrqLine>,
    lcr: u8,
    lsr: u8,
    scr: u8,
    rx_byte: Option<u8>,
}

impl UartInner {
    fn new(base: u64, irq: u32) -> Self {
        Self {
            base,
            irq,
            irq_out: None,
            lcr: 0,
            lsr: LSR_TX_EMPTY,
            scr: 0,
            rx_byte: None,
        }
    }
}

pub struct Ns16550a {
    inner: Arc<RwLock<UartInner>>,
}

impl Ns16550a {
    pub fn new(base: u64) -> Self {
        Self {
            inner: Arc::new(RwLock::new(UartInner::new(base, 10))),
        }
    }

    pub fn with_irq(base: u64, irq: u32) -> Self {
        Self {
            inner: Arc::new(RwLock::new(UartInner::new(base, irq))),
        }
    }

    pub fn clone_inner(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn set_irq_out(&self, irq_line: IrqLine) {
        println!("[UART] set_irq_out: irq_line set");
        self.inner.write().irq_out = Some(irq_line);
    }

    pub fn trigger_rx(&self) {
        let mut inner = self.inner.write();
        inner.lsr |= LSR_RX_READY;
        println!(
            "[UART] trigger_rx: lsr={:#x}, irq_out={}",
            inner.lsr,
            inner.irq_out.is_some()
        );
        if let Some(ref irq_out) = inner.irq_out {
            irq_out.set(true);
        }
    }

    pub fn trigger_rx_with_byte(&self, byte: u8) {
        let mut inner = self.inner.write();
        inner.rx_byte = Some(byte);
        inner.lsr |= LSR_RX_READY;
        if let Some(ref irq_out) = inner.irq_out {
            irq_out.set(true);
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
                if let Some(ref irq_out) = inner.irq_out {
                    irq_out.set(false);
                }
                inner.rx_byte.take().unwrap_or(0) as u64
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
