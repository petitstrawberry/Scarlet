extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use scarlet_std::sync::Mutex;

use crate::device::{DeviceFdt, FdtNodeInfo, FdtValue, IrqLine, IrqSink, MmioDevice};

pub struct PlicConfig {
    pub base: u64,
    pub num_sources: usize,
    pub num_contexts: usize,
    pub num_priorities: u32,
}

impl Default for PlicConfig {
    fn default() -> Self {
        Self {
            base: 0x0C000000,
            num_sources: 128,
            num_contexts: 2,
            num_priorities: 7,
        }
    }
}

impl PlicConfig {
    pub fn qemu_virt() -> Self {
        Self {
            base: 0x0C000000,
            num_sources: 128,
            num_contexts: 2,
            num_priorities: 7,
        }
    }

    pub fn size(&self) -> u64 {
        let enable_size = (self.num_sources.div_ceil(32) * 4) as u64;
        let enable_total = enable_size * self.num_contexts as u64;
        0x200000 + (self.num_contexts as u64 * 0x1000) + enable_total
    }
}

const PRIORITY_BASE: u64 = 0x0000;
const PENDING_BASE: u64 = 0x1000;
const ENABLE_BASE: u64 = 0x2000;
const CONTEXT_BASE: u64 = 0x200000;
const ENABLE_STRIDE: u64 = 0x80;
const CONTEXT_STRIDE: u64 = 0x1000;

const THRESHOLD_OFFSET: u64 = 0x0000;
const CLAIM_OFFSET: u64 = 0x0004;

pub struct Plic {
    config: PlicConfig,
    priority: alloc::vec::Vec<u32>,
    pending: alloc::vec::Vec<u32>,
    enable: alloc::vec::Vec<alloc::vec::Vec<u32>>,
    threshold: alloc::vec::Vec<u32>,
    claimed: alloc::vec::Vec<u32>,
    irq_out: alloc::vec::Vec<Option<IrqLine>>,
}

impl Plic {
    pub fn new(config: PlicConfig) -> Self {
        let num_sources = config.num_sources;
        let num_contexts = config.num_contexts;
        let num_words = num_sources.div_ceil(32);

        Self {
            priority: alloc::vec![0u32; num_sources],
            pending: alloc::vec![0u32; num_words],
            enable: alloc::vec![alloc::vec![0u32; num_words]; num_contexts],
            threshold: alloc::vec![0u32; num_contexts],
            claimed: alloc::vec![0u32; num_words],
            irq_out: alloc::vec![None; num_contexts],
            config,
        }
    }

    pub fn set_pending(&mut self, source: u32) {
        if source == 0 || source as usize >= self.config.num_sources {
            return;
        }
        let word = (source / 32) as usize;
        let bit = source % 32;
        self.pending[word] |= 1 << bit;
        self.update_irq();
    }

    pub fn clear_pending(&mut self, source: u32) {
        if source == 0 || source as usize >= self.config.num_sources {
            return;
        }
        let word = (source / 32) as usize;
        let bit = source % 32;
        self.pending[word] &= !(1 << bit);
        self.update_irq();
    }

    pub fn set_irq_out(&mut self, context: usize, irq: IrqLine) {
        if context < self.config.num_contexts {
            self.irq_out[context] = Some(irq);
        }
    }

    fn update_irq(&self) {
        for ctx in 0..self.config.num_contexts {
            if let Some(ref irq_out) = self.irq_out[ctx] {
                let best_id = self.highest_pending(ctx);
                irq_out.set(best_id > 0);
            }
        }
    }

    fn highest_pending(&self, context: usize) -> u32 {
        let threshold = self.threshold[context];
        let mut best_id: u32 = 0;
        let mut best_prio: u32 = 0;
        let num_words = self.config.num_sources.div_ceil(32);

        for word in 0..num_words {
            let pending = self.pending[word];
            let enabled = self.enable[context][word];
            let active = pending & enabled;

            if active == 0 {
                continue;
            }

            for bit in 0..32 {
                if active & (1 << bit) == 0 {
                    continue;
                }
                let id = (word * 32 + bit) as u32;
                let prio = self.priority[id as usize];
                if prio > threshold && (best_id == 0 || prio > best_prio) {
                    best_id = id;
                    best_prio = prio;
                }
            }
        }
        best_id
    }

    fn read_priority(&self, source: u32) -> u32 {
        if source as usize >= self.config.num_sources {
            return 0;
        }
        self.priority[source as usize]
    }

    fn write_priority(&mut self, source: u32, value: u32) {
        if source as usize >= self.config.num_sources || source == 0 {
            return;
        }
        self.priority[source as usize] = value & self.config.num_priorities;
        self.update_irq();
    }

    fn read_pending(&self, word: u32) -> u32 {
        let num_words = self.config.num_sources.div_ceil(32);
        if word as usize >= num_words {
            return 0;
        }
        self.pending[word as usize]
    }

    fn read_enable(&self, context: u32, word: u32) -> u32 {
        let num_words = self.config.num_sources.div_ceil(32);
        if context as usize >= self.config.num_contexts || word as usize >= num_words {
            return 0;
        }
        self.enable[context as usize][word as usize]
    }

    fn write_enable(&mut self, context: u32, word: u32, value: u32) {
        let num_words = self.config.num_sources.div_ceil(32);
        if context as usize >= self.config.num_contexts || word as usize >= num_words {
            return;
        }
        self.enable[context as usize][word as usize] = value;
        self.update_irq();
    }

    fn read_threshold(&self, context: u32) -> u32 {
        if context as usize >= self.config.num_contexts {
            return 0;
        }
        self.threshold[context as usize]
    }

    fn write_threshold(&mut self, context: u32, value: u32) {
        if context as usize >= self.config.num_contexts {
            return;
        }
        self.threshold[context as usize] = value & self.config.num_priorities;
        self.update_irq();
    }

    fn read_claim(&mut self, context: u32) -> u32 {
        if context as usize >= self.config.num_contexts {
            return 0;
        }
        let id = self.highest_pending(context as usize);
        if id != 0 {
            let word = (id / 32) as usize;
            let bit = id % 32;
            self.pending[word] &= !(1 << bit);
            self.claimed[word] |= 1 << bit;
            self.update_irq();
        }
        id
    }

    fn write_complete(&mut self, context: u32, id: u32) {
        if context as usize >= self.config.num_contexts
            || id == 0
            || id as usize >= self.config.num_sources
        {
            return;
        }
        let word = (id / 32) as usize;
        let bit = id % 32;
        self.claimed[word] &= !(1 << bit);
        self.update_irq();
    }
}

impl MmioDevice for Plic {
    fn base(&self) -> u64 {
        self.config.base
    }

    fn size(&self) -> u64 {
        self.config.size()
    }

    fn read(&mut self, offset: u64, _size: u8) -> u64 {
        if offset >= CONTEXT_BASE {
            let ctx_offset = offset - CONTEXT_BASE;
            let context = (ctx_offset / CONTEXT_STRIDE) as u32;
            let reg_offset = ctx_offset % CONTEXT_STRIDE;

            match reg_offset {
                THRESHOLD_OFFSET => self.read_threshold(context) as u64,
                CLAIM_OFFSET => self.read_claim(context) as u64,
                _ => 0,
            }
        } else if offset >= ENABLE_BASE {
            let enable_offset = offset - ENABLE_BASE;
            let context = (enable_offset / ENABLE_STRIDE) as u32;
            let word = ((enable_offset % ENABLE_STRIDE) / 4) as u32;
            self.read_enable(context, word) as u64
        } else if offset >= PENDING_BASE {
            let word = ((offset - PENDING_BASE) / 4) as u32;
            self.read_pending(word) as u64
        } else if offset >= PRIORITY_BASE {
            let source = ((offset - PRIORITY_BASE) / 4) as u32;
            self.read_priority(source) as u64
        } else {
            0
        }
    }

    fn write(&mut self, offset: u64, _size: u8, data: u64) {
        if offset >= CONTEXT_BASE {
            let ctx_offset = offset - CONTEXT_BASE;
            let context = (ctx_offset / CONTEXT_STRIDE) as u32;
            let reg_offset = ctx_offset % CONTEXT_STRIDE;

            match reg_offset {
                THRESHOLD_OFFSET => self.write_threshold(context, data as u32),
                CLAIM_OFFSET => self.write_complete(context, data as u32),
                _ => {}
            }
        } else if offset >= ENABLE_BASE {
            let enable_offset = offset - ENABLE_BASE;
            let context = (enable_offset / ENABLE_STRIDE) as u32;
            let word = ((enable_offset % ENABLE_STRIDE) / 4) as u32;
            self.write_enable(context, word, data as u32);
        } else if offset >= PRIORITY_BASE {
            let source = ((offset - PRIORITY_BASE) / 4) as u32;
            self.write_priority(source, data as u32);
        }
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

impl Default for Plic {
    fn default() -> Self {
        Self::new(PlicConfig::default())
    }
}

#[derive(Clone)]
pub struct PlicDevice {
    inner: Arc<Mutex<Plic>>,
}

struct PlicIrqSink {
    plic: PlicDevice,
    source: u32,
}

impl IrqSink for PlicIrqSink {
    fn set_level(&self, level: bool) {
        if level {
            self.plic.set_pending(self.source);
        } else {
            self.plic.clear_pending(self.source);
        }
    }
}

impl PlicDevice {
    pub fn new(config: PlicConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Plic::new(config))),
        }
    }

    pub fn set_pending(&self, source: u32) {
        self.inner.lock().set_pending(source);
    }

    pub fn clear_pending(&self, source: u32) {
        self.inner.lock().clear_pending(source);
    }

    pub fn get_irq_in(&self, source: u32) -> IrqLine {
        IrqLine::new(Arc::new(PlicIrqSink {
            plic: self.clone(),
            source,
        }))
    }

    pub fn set_irq_out(&self, context: usize, irq: IrqLine) {
        self.inner.lock().set_irq_out(context, irq);
    }
}

impl Default for PlicDevice {
    fn default() -> Self {
        Self::new(PlicConfig::default())
    }
}

impl MmioDevice for PlicDevice {
    fn base(&self) -> u64 {
        self.inner.lock().base()
    }

    fn size(&self) -> u64 {
        self.inner.lock().size()
    }

    fn read(&mut self, offset: u64, size: u8) -> u64 {
        self.inner.lock().read(offset, size)
    }

    fn write(&mut self, offset: u64, size: u8, data: u64) {
        self.inner.lock().write(offset, size, data)
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

impl DeviceFdt for PlicDevice {
    fn fdt_node(&self) -> Option<FdtNodeInfo> {
        let inner = self.inner.lock();
        Some(FdtNodeInfo {
            name: alloc::format!("plic@{:x}", inner.config.base),
            compatible: String::from("sifive,plic-1.0.0"),
            reg: vec![(inner.config.base, 0x600000)],
            interrupts: vec![],
            interrupt_parent: None,
            extra: vec![
                (String::from("#interrupt-cells"), FdtValue::U32(1)),
                (
                    String::from("riscv,ndev"),
                    FdtValue::U32((inner.config.num_sources - 1) as u32),
                ),
            ],
        })
    }
}
