extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::mmio::{read32, write32};
use crate::device::{
    DeviceInfo,
    manager::{DeviceManager, DriverPriority},
    platform::{PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType},
};
use crate::driver_initcall;
use crate::vm;

const REG_DATA_BASE: usize = 0x10_000;
const REG_IRQ_BASE: usize = 0x20_000;

const PINCFG_FUNC_MASK: u32 = 0b111;
const PINCFG_INPUT_ENABLE: u32 = 1 << 4;
const PINCFG_PULL_SHIFT: u32 = 5;
const PINCFG_PULL_MASK: u32 = 0b11 << PINCFG_PULL_SHIFT;

const GPIO_DATA_OUT: u32 = 1 << 0;
const GPIO_DATA_OUT_EN: u32 = 1 << 1;
const GPIO_DATA_IN: u32 = 1 << 16;

const IRQ_ENABLE: u32 = 1 << 0;
const IRQ_IS_LEVEL: u32 = 1 << 1;
const IRQ_POLARITY: u32 = 1 << 2;
const IRQ_STATUS: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpioPull {
    None,
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpioIrqTrigger {
    RisingEdge,
    FallingEdge,
    HighLevel,
    LowLevel,
}

pub struct ApplePinctrl {
    base: usize,
    npins: u32,
}

impl ApplePinctrl {
    pub fn new(base: usize, npins: u32) -> Self {
        Self { base, npins }
    }

    fn is_valid_pin(&self, pin: u32) -> bool {
        pin < self.npins
    }

    fn pincfg_offset(pin: u32) -> usize {
        (pin as usize) * 4
    }

    fn data_offset(pin: u32) -> usize {
        REG_DATA_BASE + (pin as usize) * 4
    }

    fn irq_offset(pin: u32) -> usize {
        REG_IRQ_BASE + (pin as usize) * 4
    }

    fn read_reg(&self, offset: usize) -> u32 {
        // SAFETY: `self.base` points to an ioremap'd MMIO region and offsets
        // are fixed controller register offsets.
        unsafe { read32(self.base + offset) }
    }

    fn write_reg(&self, offset: usize, value: u32) {
        // SAFETY: `self.base` points to an ioremap'd MMIO region and offsets
        // are fixed controller register offsets.
        unsafe { write32(self.base + offset, value) }
    }

    fn modify_reg(&self, offset: usize, clear_mask: u32, set_mask: u32) {
        let mut value = self.read_reg(offset);
        value &= !clear_mask;
        value |= set_mask;
        self.write_reg(offset, value);
    }

    pub fn set_direction_output(&self, pin: u32, value: bool) {
        if !self.is_valid_pin(pin) {
            return;
        }

        let offset = Self::data_offset(pin);
        let mut data = self.read_reg(offset);
        if value {
            data |= GPIO_DATA_OUT;
        } else {
            data &= !GPIO_DATA_OUT;
        }
        data |= GPIO_DATA_OUT_EN;
        self.write_reg(offset, data);
    }

    pub fn set_direction_input(&self, pin: u32) {
        if !self.is_valid_pin(pin) {
            return;
        }

        self.modify_reg(Self::data_offset(pin), GPIO_DATA_OUT_EN, 0);
        self.modify_reg(Self::pincfg_offset(pin), 0, PINCFG_INPUT_ENABLE);
    }

    pub fn set_value(&self, pin: u32, value: bool) {
        if !self.is_valid_pin(pin) {
            return;
        }

        let offset = Self::data_offset(pin);
        let mut data = self.read_reg(offset);
        if value {
            data |= GPIO_DATA_OUT;
        } else {
            data &= !GPIO_DATA_OUT;
        }
        self.write_reg(offset, data);
    }

    pub fn get_value(&self, pin: u32) -> bool {
        if !self.is_valid_pin(pin) {
            return false;
        }

        (self.read_reg(Self::data_offset(pin)) & GPIO_DATA_IN) != 0
    }

    pub fn set_pull(&self, pin: u32, pull: GpioPull) {
        if !self.is_valid_pin(pin) {
            return;
        }

        let pull_bits = match pull {
            GpioPull::None => 0,
            GpioPull::Down => 1,
            GpioPull::Up => 2,
        };

        self.modify_reg(
            Self::pincfg_offset(pin),
            PINCFG_PULL_MASK,
            pull_bits << PINCFG_PULL_SHIFT,
        );
    }

    pub fn set_function(&self, pin: u32, func: u8) {
        if !self.is_valid_pin(pin) {
            return;
        }

        self.modify_reg(
            Self::pincfg_offset(pin),
            PINCFG_FUNC_MASK,
            (func as u32) & PINCFG_FUNC_MASK,
        );
    }

    pub fn enable_irq(&self, pin: u32, trigger: GpioIrqTrigger) {
        if !self.is_valid_pin(pin) {
            return;
        }

        let mut irq = IRQ_ENABLE;
        match trigger {
            GpioIrqTrigger::RisingEdge => {}
            GpioIrqTrigger::FallingEdge => {
                irq |= IRQ_POLARITY;
            }
            GpioIrqTrigger::HighLevel => {
                irq |= IRQ_IS_LEVEL;
            }
            GpioIrqTrigger::LowLevel => {
                irq |= IRQ_IS_LEVEL | IRQ_POLARITY;
            }
        }

        let offset = Self::irq_offset(pin);
        self.write_reg(offset, IRQ_STATUS);
        self.modify_reg(offset, IRQ_IS_LEVEL | IRQ_POLARITY | IRQ_ENABLE, irq);
    }

    pub fn disable_irq(&self, pin: u32) {
        if !self.is_valid_pin(pin) {
            return;
        }

        self.modify_reg(Self::irq_offset(pin), IRQ_ENABLE, 0);
    }

    pub fn ack_irq(&self, pin: u32) {
        if !self.is_valid_pin(pin) {
            return;
        }

        self.write_reg(Self::irq_offset(pin), IRQ_STATUS);
    }
}

static PINCTRL_REGISTRY: Mutex<Vec<Arc<ApplePinctrl>>> = Mutex::new(Vec::new());

pub fn get_pinctrl(index: usize) -> Option<Arc<ApplePinctrl>> {
    PINCTRL_REGISTRY.lock().get(index).map(Arc::clone)
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resources: Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let resource = mem_resources
        .first()
        .ok_or("apple-pinctrl: no memory resource")?;

    let paddr = resource.start;
    let size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|v| v.checked_add(1))
        .ok_or("apple-pinctrl: invalid memory resource")?;

    let base = vm::ioremap(paddr, size).map_err(|_| "apple-pinctrl: ioremap failed")?;

    let npins = device
        .property("apple,npins")
        .and_then(|property| property.as_usize())
        .ok_or("apple-pinctrl: missing apple,npins")? as u32;

    let pinctrl = Arc::new(ApplePinctrl::new(base, npins));
    PINCTRL_REGISTRY.lock().push(pinctrl);

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_pinctrl_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-pinctrl",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-pinctrl",
            "apple,t8112-pinctrl",
            "apple,pinctrl"
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

driver_initcall!(register_apple_pinctrl_driver);
