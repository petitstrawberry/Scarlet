extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use scarlet_std::sync::RwLock;

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

/// NS16550A UART with lock-free register access.
///
/// Concurrency:
/// - MMIO thread: read/write all registers
/// - UART input thread: write rx_byte, set lsr RX_READY, trigger irq
///
/// Atomic fields handle the race between these threads without locks.
pub struct Ns16550a {
    base: u64,
    irq: u32,
    lsr: AtomicU8,
    rx_byte: AtomicU8,
    rx_valid: AtomicBool,
    lcr: AtomicU8,
    scr: AtomicU8,
    /// Set once during init, read thereafter. RwLock for the one-time write.
    irq_out: RwLock<Option<IrqLine>>,
}

impl Ns16550a {
    pub fn new(base: u64) -> Self {
        Self {
            base,
            irq: 10,
            lsr: AtomicU8::new(LSR_TX_EMPTY),
            rx_byte: AtomicU8::new(0),
            rx_valid: AtomicBool::new(false),
            lcr: AtomicU8::new(0),
            scr: AtomicU8::new(0),
            irq_out: RwLock::new(None),
        }
    }

    pub fn with_irq(base: u64, irq: u32) -> Self {
        Self {
            base,
            irq,
            lsr: AtomicU8::new(LSR_TX_EMPTY),
            rx_byte: AtomicU8::new(0),
            rx_valid: AtomicBool::new(false),
            lcr: AtomicU8::new(0),
            scr: AtomicU8::new(0),
            irq_out: RwLock::new(None),
        }
    }

    pub fn clone_inner(&self) -> Arc<Self> {
        Arc::new(Self {
            base: self.base,
            irq: self.irq,
            lsr: AtomicU8::new(self.lsr.load(Ordering::Relaxed)),
            rx_byte: AtomicU8::new(self.rx_byte.load(Ordering::Relaxed)),
            rx_valid: AtomicBool::new(self.rx_valid.load(Ordering::Relaxed)),
            lcr: AtomicU8::new(self.lcr.load(Ordering::Relaxed)),
            scr: AtomicU8::new(self.scr.load(Ordering::Relaxed)),
            irq_out: RwLock::new(self.irq_out.read().clone()),
        })
    }

    pub fn set_irq_out(&self, irq_line: IrqLine) {
        *self.irq_out.write() = Some(irq_line);
    }

    pub fn trigger_rx(&self) {
        self.lsr.fetch_or(LSR_RX_READY, Ordering::Release);
        if let Some(ref irq_out) = *self.irq_out.read() {
            irq_out.set(true);
        }
    }

    pub fn trigger_rx_with_byte(&self, byte: u8) {
        self.rx_byte.store(byte, Ordering::Relaxed);
        self.rx_valid.store(true, Ordering::Relaxed);
        self.lsr.fetch_or(LSR_RX_READY, Ordering::Release);
        if let Some(ref irq_out) = *self.irq_out.read() {
            irq_out.set(true);
        }
    }
}

impl MmioDevice for Ns16550a {
    fn base(&self) -> u64 {
        self.base
    }

    fn size(&self) -> u64 {
        0x1000
    }

    fn read(&self, offset: u64, _size: u8) -> u64 {
        match offset {
            RBR => {
                self.lsr.fetch_and(!LSR_RX_READY, Ordering::Acquire);
                if let Some(ref irq_out) = *self.irq_out.read() {
                    irq_out.set(false);
                }
                let byte = if self.rx_valid.swap(false, Ordering::Acquire) {
                    self.rx_byte.load(Ordering::Relaxed)
                } else {
                    0
                };
                byte as u64
            }
            IER => 0,
            IIR => 0x01,
            LCR => self.lcr.load(Ordering::Relaxed) as u64,
            MCR => 0,
            LSR => self.lsr.load(Ordering::Acquire) as u64,
            MSR => 0,
            SCR => self.scr.load(Ordering::Relaxed) as u64,
            _ => 0,
        }
    }

    fn write(&self, offset: u64, _size: u8, data: u64) {
        let byte = data as u8;
        match offset {
            THR => {
                print!("{}", byte as char);
            }
            IER => {}
            FCR => {}
            LCR => {
                self.lcr.store(byte, Ordering::Relaxed);
            }
            MCR => {}
            SCR => {
                self.scr.store(byte, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl DeviceFdt for Ns16550a {
    fn fdt_node(&self) -> Option<FdtNodeInfo> {
        Some(FdtNodeInfo {
            name: alloc::format!("serial@{:x}", self.base),
            compatible: String::from("ns16550a"),
            reg: vec![(self.base, 0x100)],
            interrupts: vec![self.irq],
            interrupt_parent: None,
            extra: vec![(String::from("clock-frequency"), FdtValue::U32(3686400))],
        })
    }
}
