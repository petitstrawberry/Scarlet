#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        clk::{Clk, ClkError, ClkHandle, ClkProvider},
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
};

const NCO_CHANNEL_STRIDE: usize = 0x4000;
const REG_CTRL: usize = 0x00;
const REG_DIV: usize = 0x04;
const REG_INC1: usize = 0x08;
const REG_INC2: usize = 0x0c;
const REG_ACCINIT: usize = 0x10;

const CTRL_ENABLE: u32 = 1 << 31;
const DIV_FINE_MASK: u32 = 0x3;
const DIV_COARSE_MASK: u32 = 0x7ff << 2;
const DIV_COARSE_SHIFT: u32 = 2;

const LFSR_POLY: u32 = 0xa01;
const LFSR_INIT: u32 = 0x7ff;
const LFSR_PERIOD: usize = (1 << 11) - 1;
const LFSR_TABLE_SIZE: usize = 1 << 11;
const COARSE_DIV_OFFSET: usize = 2;
const ACC_INIT_NEUTRAL: u32 = 1 << 31;

const APPLE_NCO_CLOCK_CELLS: usize = 1;
const APPLE_NCO_T8103_CLOCKS: usize = 6;
const APPLE_NCO_T6000_CLOCKS: usize = 4;

struct AppleNcoClock {
    index: u32,
    base: usize,
    parent: Option<ClkHandle>,
    lock: Mutex<()>,
}

impl AppleNcoClock {
    fn new(index: u32, base: usize, parent: Option<ClkHandle>) -> Self {
        Self {
            index,
            base,
            parent,
            lock: Mutex::new(()),
        }
    }

    fn read_reg(&self, offset: usize) -> u32 {
        // SAFETY: `self.base` is the ioremap'd register window for this NCO
        // channel and offsets are fixed NCO channel register offsets.
        unsafe { mmio::read32(self.base + offset) }
    }

    fn write_reg(&self, offset: usize, value: u32) {
        // SAFETY: `self.base` is the ioremap'd register window for this NCO
        // channel and offsets are fixed NCO channel register offsets.
        unsafe { mmio::write32(self.base + offset, value) }
    }

    fn enable_nolock(&self) {
        let value = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, value | CTRL_ENABLE);
    }

    fn disable_nolock(&self) {
        let value = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, value & !CTRL_ENABLE);
    }

    fn is_enabled_nolock(&self) -> bool {
        self.read_reg(REG_CTRL) & CTRL_ENABLE != 0
    }

    fn lfsr_state_for_coarse(coarse: usize) -> Option<u32> {
        if coarse == 0 {
            return Some(0);
        }

        let mut state = LFSR_INIT;
        for index in (1..=LFSR_PERIOD).rev() {
            state = if state & 1 != 0 {
                (state >> 1) ^ (LFSR_POLY >> 1)
            } else {
                state >> 1
            };
            if index == coarse {
                return Some(state);
            }
        }

        None
    }

    fn coarse_for_lfsr_state(target: u32) -> Option<usize> {
        if target == 0 {
            return Some(0);
        }

        let mut state = LFSR_INIT;
        for index in (1..=LFSR_PERIOD).rev() {
            state = if state & 1 != 0 {
                (state >> 1) ^ (LFSR_POLY >> 1)
            } else {
                state >> 1
            };
            if state == target {
                return Some(index);
            }
        }

        None
    }

    fn div_out_of_range(div: usize) -> bool {
        let coarse = div / 4;
        coarse < COARSE_DIV_OFFSET || coarse >= COARSE_DIV_OFFSET + LFSR_TABLE_SIZE
    }

    fn translate_div(div: usize) -> Option<u32> {
        if Self::div_out_of_range(div) {
            return None;
        }

        let coarse = div / 4 - COARSE_DIV_OFFSET;
        let coarse = Self::lfsr_state_for_coarse(coarse)?;
        Some(((coarse << DIV_COARSE_SHIFT) & DIV_COARSE_MASK) | (div as u32 & DIV_FINE_MASK))
    }

    fn translate_div_inv(value: u32) -> Option<usize> {
        let coarse_state = (value & DIV_COARSE_MASK) >> DIV_COARSE_SHIFT;
        let fine = (value & DIV_FINE_MASK) as usize;
        let coarse = Self::coarse_for_lfsr_state(coarse_state)? + COARSE_DIV_OFFSET;
        Some(coarse * 4 + fine)
    }
}

impl Clk for AppleNcoClock {
    fn name(&self) -> &'static str {
        "apple-nco"
    }

    fn enable(&self) -> Result<(), ClkError> {
        let _guard = self.lock.lock();
        self.enable_nolock();
        Ok(())
    }

    fn disable(&self) {
        let _guard = self.lock.lock();
        self.disable_nolock();
    }

    fn is_enabled(&self) -> bool {
        self.is_enabled_nolock()
    }

    fn recalc_rate(&self, parent_rate: u64) -> u64 {
        if parent_rate == 0 {
            return 0;
        }

        let Some(div) = Self::translate_div_inv(self.read_reg(REG_DIV)) else {
            return 0;
        };
        let inc1 = self.read_reg(REG_INC1);
        let inc2 = self.read_reg(REG_INC2);
        if inc1 >= ACC_INIT_NEUTRAL || inc2 < ACC_INIT_NEUTRAL || (inc1 == 0 && inc2 == 0) {
            return 0;
        }

        let incbase = inc1.wrapping_sub(inc2) as u128;
        let numerator = parent_rate as u128 * 2 * incbase;
        let denominator = div as u128 * incbase + inc1 as u128;
        if denominator == 0 {
            0
        } else {
            (numerator / denominator) as u64
        }
    }

    fn round_rate(&self, rate: u64, parent_rate: u64) -> Result<u64, ClkError> {
        if parent_rate == 0 || rate == 0 {
            return Err(ClkError::InvalidRate);
        }

        let lo = parent_rate / (COARSE_DIV_OFFSET as u64 + LFSR_TABLE_SIZE as u64) + 1;
        let hi = parent_rate / COARSE_DIV_OFFSET as u64;
        Ok(rate.clamp(lo, hi))
    }

    fn set_rate(&self, rate: u64, parent_rate: u64) -> Result<u64, ClkError> {
        if parent_rate == 0 || rate == 0 {
            return Err(ClkError::InvalidRate);
        }
        if rate > u64::from(u32::MAX) {
            return Err(ClkError::InvalidRate);
        }

        let parent_rate_doubled = parent_rate.checked_mul(2).ok_or(ClkError::InvalidRate)?;
        let div = (parent_rate_doubled / rate) as usize;
        let Some(translated_div) = Self::translate_div(div) else {
            return Err(ClkError::InvalidRate);
        };

        let inc1 = (parent_rate_doubled - div as u64 * rate) as u32;
        let inc2 = inc1.wrapping_sub(rate as u32);
        let _guard = self.lock.lock();
        let was_enabled = self.is_enabled_nolock();
        self.disable_nolock();
        self.write_reg(REG_DIV, translated_div);
        self.write_reg(REG_INC1, inc1);
        self.write_reg(REG_INC2, inc2);
        self.write_reg(REG_ACCINIT, ACC_INIT_NEUTRAL);
        if was_enabled {
            self.enable_nolock();
        }

        Ok(self.recalc_rate(parent_rate))
    }

    fn parent(&self) -> Option<ClkHandle> {
        self.parent.clone()
    }
}

struct AppleNcoProvider {
    base: usize,
    size: usize,
    clocks: Vec<ClkHandle>,
}

impl AppleNcoProvider {
    fn new(base: usize, size: usize, clock_count: usize, parent: Option<ClkHandle>) -> Self {
        let mut clocks = Vec::new();
        for index in 0..clock_count {
            let channel_base = base + NCO_CHANNEL_STRIDE * index;
            clocks.push(ClkHandle::new(Arc::new(AppleNcoClock::new(
                index as u32,
                channel_base,
                parent.clone(),
            ))));
        }

        Self { base, size, clocks }
    }
}

impl ClkProvider for AppleNcoProvider {
    fn name(&self) -> &'static str {
        "apple-nco"
    }

    fn clock_cells(&self) -> usize {
        APPLE_NCO_CLOCK_CELLS
    }

    fn get_clk(&self, spec: &[u32]) -> Result<ClkHandle, ClkError> {
        if spec.len() != APPLE_NCO_CLOCK_CELLS {
            return Err(ClkError::InvalidSpecifier);
        }

        let index = spec[0] as usize;
        self.clocks
            .get(index)
            .cloned()
            .ok_or(ClkError::ClockNotFound)
    }
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|value| value as u32)
        .ok_or("apple-nco: missing phandle")
}

fn clock_count(device: &PlatformDeviceInfo) -> usize {
    let compatible = device.compatible();
    if compatible
        .iter()
        .any(|entry| *entry == "apple,t6000-nco" || *entry == "apple,t6020-nco")
    {
        APPLE_NCO_T6000_CLOCKS
    } else {
        APPLE_NCO_T8103_CLOCKS
    }
}

fn resolve_parent_clock(device: &PlatformDeviceInfo) -> Result<Option<ClkHandle>, &'static str> {
    if device.property("clocks").is_none() {
        return Ok(None);
    }

    let parent = DeviceManager::get_manager()
        .resolve_clk(device, "ref")
        .map_err(|_| "apple-nco: failed to resolve reference clock")?;
    parent
        .prepare_enable()
        .map_err(|_| "apple-nco: failed to enable reference clock")?;
    Ok(Some(parent))
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-nco: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;
    let base = scarlet::vm::ioremap(paddr, size).map_err(|_| "apple-nco: ioremap failed")?;
    let phandle = read_phandle(device)?;
    let clock_cells = device
        .property("#clock-cells")
        .and_then(|property| property.as_usize())
        .unwrap_or(APPLE_NCO_CLOCK_CELLS);
    if clock_cells != APPLE_NCO_CLOCK_CELLS {
        return Err("apple-nco: unsupported #clock-cells");
    }

    let parent = resolve_parent_clock(device)?;
    let count = clock_count(device);
    let provider = Arc::new(AppleNcoProvider::new(base, size, count, parent));
    DeviceManager::get_manager().register_clk_provider(phandle, provider);

    early_println!(
        "[apple-nco] registered {} at paddr={:#x}, base={:#x}, clocks={}",
        device.name(),
        paddr,
        base,
        count
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-nco",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-nco",
            "apple,t8112-nco",
            "apple,t6000-nco",
            "apple,t6020-nco",
            "apple,nco",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_NCO_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
