use spin::Mutex;

use crate::hypervisor::mmio::VirtualMmioDevice;

const GICD_SIZE: u64 = 0x10000;
const GICR_SIZE: u64 = 0x20000;
const REG_WORDS: usize = 4;
const CONFIG_WORDS: usize = REG_WORDS * 2;
const GIC_IIDR_ARM: u32 = 0x43B;
const GIC_PIDR2_ARCH_GICV3: u32 = 0x30;
const GICD_CTLR_DS: u32 = 1 << 6;
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;
const GIC_ICFGR_EDGE_MASK: u32 = 0xaaaa_aaaa;

struct VgicDistState {
    ctlr: u32,
    typer: u32,
    iidr: u32,
    enabled: [u32; REG_WORDS],
    group: [u32; REG_WORDS],
    pending: [u32; REG_WORDS],
    active: [u32; REG_WORDS],
    config: [u32; CONFIG_WORDS],
}

pub struct VgicDist {
    base: u64,
    num_lrs: usize,
    state: Mutex<VgicDistState>,
}

impl VgicDist {
    pub fn new(base: u64, nr_irqs: u32, num_lrs: usize) -> Self {
        let typer = (nr_irqs / 32).saturating_sub(1) | (1 << 10);
        Self {
            base,
            num_lrs,
            state: Mutex::new(VgicDistState {
                ctlr: 0,
                typer,
                iidr: GIC_IIDR_ARM,
                enabled: [0; REG_WORDS],
                group: [0; REG_WORDS],
                pending: [0; REG_WORDS],
                active: [0; REG_WORDS],
                config: [0; CONFIG_WORDS],
            }),
        }
    }

    fn read_bitmap(offset: u64, base: u64, values: &[u32; REG_WORDS]) -> u32 {
        let index = ((offset - base) / 4) as usize;
        *values.get(index).unwrap_or(&0)
    }

    fn read_config(offset: u64, base: u64, values: &[u32; CONFIG_WORDS]) -> u32 {
        let index = ((offset - base) / 4) as usize;
        *values.get(index).unwrap_or(&0)
    }

    fn write_config(index: usize, value: u32, values: &mut [u32; CONFIG_WORDS]) {
        if index < CONFIG_WORDS {
            values[index] = value & GIC_ICFGR_EDGE_MASK;
        }
    }

    fn set_enabled_bits(index: usize, bits: u32, enabled: &mut [u32; REG_WORDS]) {
        if index >= REG_WORDS {
            return;
        }

        enabled[index] |= bits;
    }

    fn set_pending_bits(
        &self,
        index: usize,
        bits: u32,
        enabled: &[u32; REG_WORDS],
        pending: &mut [u32; REG_WORDS],
    ) {
        if index >= REG_WORDS {
            return;
        }

        pending[index] |= bits;
        let injectable = bits & enabled[index];
        for bit in 0..32 {
            let mask = 1u32 << bit;
            if (injectable & mask) != 0 {
                let intid = (index * 32 + bit) as u32;
                let _ = super::vgic::inject_virq(self.num_lrs, intid, 0xa0, true);
            }
        }
    }

    fn clear_enabled_bits(&self, index: usize, bits: u32, enabled: &mut [u32; REG_WORDS]) {
        if index >= REG_WORDS {
            return;
        }

        let prev = enabled[index];
        let cleared = prev & bits;
        enabled[index] = prev & !bits;

        for bit in 0..32 {
            let mask = 1u32 << bit;
            if (cleared & mask) != 0 {
                let intid = (index * 32 + bit) as u32;
                let _ = super::vgic::clear_virq(self.num_lrs, intid);
            }
        }
    }
}

impl VirtualMmioDevice for VgicDist {
    fn read(&self, offset: u64, _size: u8) -> u64 {
        match offset {
            0x0000 => (self.state.lock().ctlr | GICD_CTLR_DS) as u64,
            0x0004 => self.state.lock().typer as u64,
            0x0008 => self.state.lock().iidr as u64,
            0x0010 => 0,
            0x0080..=0x008c if offset % 4 == 0 => {
                let state = self.state.lock();
                Self::read_bitmap(offset, 0x0080, &state.group) as u64
            }
            0x0100..=0x010c if offset % 4 == 0 => {
                let state = self.state.lock();
                Self::read_bitmap(offset, 0x0100, &state.enabled) as u64
            }
            0x0180..=0x018c if offset % 4 == 0 => {
                let state = self.state.lock();
                Self::read_bitmap(offset, 0x0180, &state.enabled) as u64
            }
            0x0200..=0x020c if offset % 4 == 0 => {
                let state = self.state.lock();
                Self::read_bitmap(offset, 0x0200, &state.pending) as u64
            }
            0x0300..=0x030c if offset % 4 == 0 => {
                let state = self.state.lock();
                Self::read_bitmap(offset, 0x0300, &state.active) as u64
            }
            0x0400..=0x047f => 0xa0,
            0x0c00..=0x0c7c if offset % 4 == 0 => {
                let state = self.state.lock();
                Self::read_config(offset, 0x0c00, &state.config) as u64
            }
            0x6100..=0x61f8 if offset % 8 == 0 => 0,
            0xffe8 => GIC_PIDR2_ARCH_GICV3 as u64,
            _ => 0,
        }
    }

    fn write(&self, offset: u64, _size: u8, value: u64) {
        let value = value as u32;
        let mut state = self.state.lock();

        match offset {
            0x0000 => state.ctlr = value,
            0x0080..=0x008c if offset % 4 == 0 => {
                let index = ((offset - 0x0080) / 4) as usize;
                if index < REG_WORDS {
                    state.group[index] = value;
                }
            }
            0x0100..=0x010c if offset % 4 == 0 => {
                let index = ((offset - 0x0100) / 4) as usize;
                Self::set_enabled_bits(index, value, &mut state.enabled);
            }
            0x0180..=0x018c if offset % 4 == 0 => {
                let index = ((offset - 0x0180) / 4) as usize;
                self.clear_enabled_bits(index, value, &mut state.enabled);
            }
            0x0200..=0x020c if offset % 4 == 0 => {
                let index = ((offset - 0x0200) / 4) as usize;
                let enabled = state.enabled;
                self.set_pending_bits(index, value, &enabled, &mut state.pending);
            }
            0x0280..=0x028c if offset % 4 == 0 => {
                let index = ((offset - 0x0280) / 4) as usize;
                if index < REG_WORDS {
                    state.pending[index] &= !value;
                }
            }
            0x0300..=0x030c if offset % 4 == 0 => {
                let index = ((offset - 0x0300) / 4) as usize;
                if index < REG_WORDS {
                    state.active[index] &= !value;
                }
            }
            0x0400..=0x047f => {}
            0x0c00..=0x0c7c if offset % 4 == 0 => {
                let index = ((offset - 0x0c00) / 4) as usize;
                Self::write_config(index, value, &mut state.config);
            }
            0x6100..=0x61f8 if offset % 8 == 0 => {}
            _ => {}
        }
    }

    fn addr_range(&self) -> (u64, u64) {
        (self.base, GICD_SIZE)
    }
}

struct VgicRedistState {
    typer: u64,
    waker: u32,
    sgi_enabled: u32,
    sgi_group: u32,
    sgi_pending: u32,
    sgi_active: u32,
    sgi_config: [u32; 2],
}

pub struct VgicRedist {
    base: u64,
    num_lrs: usize,
    state: Mutex<VgicRedistState>,
}

impl VgicRedist {
    pub fn new(base: u64, num_lrs: usize) -> Self {
        Self {
            base,
            num_lrs,
            state: Mutex::new(VgicRedistState {
                typer: 1 << 4,
                waker: GICR_WAKER_PROCESSOR_SLEEP | GICR_WAKER_CHILDREN_ASLEEP,
                sgi_enabled: 0,
                sgi_group: 0,
                sgi_pending: 0,
                sgi_active: 0,
                sgi_config: [GIC_ICFGR_EDGE_MASK, 0],
            }),
        }
    }

    fn set_enabled_bits(bits: u32, enabled: &mut u32) {
        *enabled |= bits;
    }

    fn set_pending_bits(&self, bits: u32, enabled: u32, pending: &mut u32) {
        *pending |= bits;
        let injectable = bits & enabled;
        for bit in 0..32 {
            let mask = 1u32 << bit;
            if (injectable & mask) != 0 {
                let _ = super::vgic::inject_virq(self.num_lrs, bit as u32, 0xa0, true);
            }
        }
    }

    fn clear_enabled_bits(&self, bits: u32, enabled: &mut u32) {
        let prev = *enabled;
        let cleared = prev & bits;
        *enabled = prev & !bits;

        for bit in 0..32 {
            let mask = 1u32 << bit;
            if (cleared & mask) != 0 {
                let _ = super::vgic::clear_virq(self.num_lrs, bit as u32);
            }
        }
    }
}

impl VirtualMmioDevice for VgicRedist {
    fn read(&self, offset: u64, size: u8) -> u64 {
        let state = self.state.lock();
        match offset {
            0x0000 => 0,
            0x0004 => GIC_IIDR_ARM as u64,
            0x0008 => {
                if size > 4 {
                    state.typer
                } else {
                    (state.typer as u32) as u64
                }
            }
            0x0014 => state.waker as u64,
            0x0024 => 2,
            0x00c0 => 0,
            0xffe8 => GIC_PIDR2_ARCH_GICV3 as u64,
            0x10080 => state.sgi_group as u64,
            0x10100 => state.sgi_enabled as u64,
            0x10180 => state.sgi_enabled as u64,
            0x10200..=0x1021f => 0,
            0x10280 => state.sgi_pending as u64,
            0x10300 => state.sgi_active as u64,
            0x10400..=0x1047f => 0xa0,
            0x10c00..=0x10c04 if offset % 4 == 0 => {
                let index = ((offset - 0x10c00) / 4) as usize;
                state.sgi_config[index] as u64
            }
            0x10c08..=0x10c7c if offset % 4 == 0 => 0,
            _ => 0,
        }
    }

    fn write(&self, offset: u64, _size: u8, value: u64) {
        let value = value as u32;
        let mut state = self.state.lock();

        match offset {
            0x0000 => {}
            0x0014 => {
                state.waker = if (value & GICR_WAKER_PROCESSOR_SLEEP) != 0 {
                    GICR_WAKER_PROCESSOR_SLEEP | GICR_WAKER_CHILDREN_ASLEEP
                } else {
                    0
                };
            }
            0x0024 => {}
            0x10080 => state.sgi_group = value,
            0x10100 => Self::set_enabled_bits(value, &mut state.sgi_enabled),
            0x10180 => self.clear_enabled_bits(value, &mut state.sgi_enabled),
            0x10200..=0x1021f => {
                let enabled = state.sgi_enabled;
                self.set_pending_bits(value, enabled, &mut state.sgi_pending);
            }
            0x10280 => state.sgi_pending &= !value,
            0x10300 => state.sgi_active &= !value,
            0x10400..=0x1047f => {}
            0x10c00..=0x10c04 if offset % 4 == 0 => {
                let index = ((offset - 0x10c00) / 4) as usize;
                state.sgi_config[index] = value & GIC_ICFGR_EDGE_MASK;
            }
            0x10c08..=0x10c7c if offset % 4 == 0 => {}
            _ => {}
        }
    }

    fn addr_range(&self) -> (u64, u64) {
        (self.base, GICR_SIZE)
    }
}
