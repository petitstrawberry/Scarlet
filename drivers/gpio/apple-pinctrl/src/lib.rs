#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::arch::mmio::{read32, write32};
use scarlet::device::{
    events::InterruptCapableDevice,
    gpio::{GpioController, GpioIrqTrigger, GpioPull},
    manager::{DeviceManager, DriverPriority},
    platform::{PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType},
};
use scarlet::interrupt::{InterruptId, InterruptManager, InterruptResult};
use scarlet::vm;

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

pub struct ApplePinctrl {
    base: usize,
    npins: u32,
    parent_irqs: Mutex<Vec<InterruptId>>,
    irq_handlers: Mutex<BTreeMap<u32, Arc<dyn InterruptCapableDevice>>>,
}

impl ApplePinctrl {
    pub fn new(base: usize, npins: u32) -> Self {
        Self {
            base,
            npins,
            parent_irqs: Mutex::new(Vec::new()),
            irq_handlers: Mutex::new(BTreeMap::new()),
        }
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

    fn register_parent_irqs(
        pinctrl: &Arc<Self>,
        device: &PlatformDeviceInfo,
    ) -> Result<(), &'static str> {
        let irq_resources: Vec<_> = device
            .get_resources()
            .iter()
            .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::IRQ))
            .collect();

        if irq_resources.is_empty() {
            return Err("apple-pinctrl: no IRQ resources for parent interrupt lines");
        }

        for irq_res in &irq_resources {
            let irq_id = if let Some(ref md) = irq_res.irq_metadata {
                md.irq_number
            } else {
                irq_res.start as u32
            };

            InterruptManager::global()
                .register_interrupt_device(irq_id, pinctrl.clone())
                .map_err(|_| "apple-pinctrl: failed to register parent IRQ handler")?;

            InterruptManager::global()
                .enable_external_interrupt(irq_id, 0)
                .map_err(|_| "apple-pinctrl: failed to enable parent IRQ")?;

            pinctrl.parent_irqs.lock().push(irq_id);
        }

        Ok(())
    }
}

impl GpioController for ApplePinctrl {
    fn set_direction_output(&self, pin: u32, value: bool) {
        Self::set_direction_output(self, pin, value)
    }
    fn set_direction_input(&self, pin: u32) {
        Self::set_direction_input(self, pin)
    }
    fn set_value(&self, pin: u32, value: bool) {
        Self::set_value(self, pin, value)
    }
    fn get_value(&self, pin: u32) -> bool {
        Self::get_value(self, pin)
    }
    fn set_pull(&self, pin: u32, pull: GpioPull) {
        Self::set_pull(self, pin, pull)
    }
    fn set_function(&self, pin: u32, func: u8) {
        Self::set_function(self, pin, func)
    }
    fn enable_irq(&self, pin: u32, trigger: GpioIrqTrigger) {
        Self::enable_irq(self, pin, trigger)
    }
    fn disable_irq(&self, pin: u32) {
        Self::disable_irq(self, pin)
    }
    fn ack_irq(&self, pin: u32) {
        Self::ack_irq(self, pin)
    }

    fn request_irq(
        &self,
        pin: u32,
        trigger: GpioIrqTrigger,
        handler: Arc<dyn InterruptCapableDevice>,
    ) -> bool {
        if !self.is_valid_pin(pin) {
            return false;
        }

        self.enable_irq(pin, trigger);
        self.irq_handlers.lock().insert(pin, handler);
        true
    }

    fn free_irq(&self, pin: u32) {
        if !self.is_valid_pin(pin) {
            return;
        }

        self.irq_handlers.lock().remove(&pin);
        self.disable_irq(pin);
    }
}

impl InterruptCapableDevice for ApplePinctrl {
    fn handle_interrupt(&self) -> InterruptResult<()> {
        let handlers = self.irq_handlers.lock();

        for pin in 0..self.npins {
            let reg = self.read_reg(Self::irq_offset(pin));
            if (reg & IRQ_STATUS) == 0 {
                continue;
            }

            if let Some(handler) = handlers.get(&pin) {
                let _ = handler.handle_interrupt();
            }

            self.ack_irq(pin);
        }

        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        self.parent_irqs.lock().first().copied()
    }
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

    let phandle = device
        .property("phandle")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .ok_or("apple-pinctrl: no phandle")?;

    let pinctrl: Arc<ApplePinctrl> = Arc::new(ApplePinctrl::new(base, npins));

    ApplePinctrl::register_parent_irqs(&pinctrl, device)?;

    DeviceManager::get_manager().register_gpio_controller(phandle, pinctrl);

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

scarlet::driver_initcall!(register_apple_pinctrl_driver);

#[used]
static SCARLET_DRIVER_APPLE_PINCTRL_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
