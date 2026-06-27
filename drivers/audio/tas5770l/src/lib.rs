#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
    device::{
        DeviceInfo,
        audio::{
            AUDIO_PCM_FORMAT_S16LE, AUDIO_PCM_FORMAT_S24LE3, AUDIO_PCM_FORMAT_S32LE, AudioCodec,
            AudioPcmParams,
        },
        fdt::FdtManager,
        gpio::GpioController,
        i2c::{I2cAddress, I2cBus, I2cError, I2cMessage},
        manager::{DeviceManager, DriverPriority, probe_defer},
        platform::{PlatformDeviceDriver, PlatformDeviceInfo},
    },
    early_println,
    time::udelay,
};

const TAS5770L_MAX_7BIT_ADDRESS: usize = 0x7f;
const TAS5770L_SOUND_DAI_CELLS: usize = 0;
const TAS5770L_DEFAULT_RATE: u32 = 48_000;
const TAS5770L_DEFAULT_TX_MASK: u32 = 0x3;
const TAS5770L_DEFAULT_SLOTS: usize = 2;
const TAS5770L_DEFAULT_SLOT_WIDTH: usize = 32;

const TAS2770_SW_RST: u8 = 0x01;
const TAS2770_RST: u8 = 1 << 0;
const TAS2770_PWR_CTRL: u8 = 0x02;
const TAS2770_PWR_CTRL_MASK: u8 = 0x03;
const TAS2770_PWR_CTRL_ACTIVE: u8 = 0x00;
const TAS2770_PWR_CTRL_MUTE: u8 = 0x01;
const TAS2770_PWR_CTRL_SHUTDOWN: u8 = 0x02;
const TAS2770_TDM_CFG_REG0: u8 = 0x0a;
const TAS2770_TDM_CFG_REG0_SMP_MASK: u8 = 1 << 5;
const TAS2770_TDM_CFG_REG0_SMP_48KHZ: u8 = 0x00;
const TAS2770_TDM_CFG_REG0_SMP_44_1KHZ: u8 = 1 << 5;
const TAS2770_TDM_CFG_REG0_RATE_MASK: u8 = 0x0e;
const TAS2770_TDM_CFG_REG0_RATE_44_1_48KHZ: u8 = 0x06;
const TAS2770_TDM_CFG_REG0_RATE_88_2_96KHZ: u8 = 0x08;
const TAS2770_TDM_CFG_REG0_RATE_176_4_192KHZ: u8 = 0x0a;
const TAS2770_TDM_CFG_REG0_FPOL_MASK: u8 = 1 << 0;
const TAS2770_TDM_CFG_REG0_FPOL_FALLING: u8 = 1;
const TAS2770_TDM_CFG_REG1: u8 = 0x0b;
const TAS2770_TDM_CFG_REG1_START_SLOT_MASK: u8 = 0x3e;
const TAS2770_TDM_CFG_REG1_START_SLOT_SHIFT: u8 = 1;
const TAS2770_TDM_CFG_REG1_RX_EDGE_MASK: u8 = 1 << 0;
const TAS2770_TDM_CFG_REG1_RX_FALLING: u8 = 1;
const TAS2770_TDM_CFG_REG2: u8 = 0x0c;
const TAS2770_TDM_CFG_REG2_RXW_MASK: u8 = 0x0c;
const TAS2770_TDM_CFG_REG2_RXW_16BITS: u8 = 0x00;
const TAS2770_TDM_CFG_REG2_RXW_24BITS: u8 = 0x08;
const TAS2770_TDM_CFG_REG2_RXW_32BITS: u8 = 0x0c;
const TAS2770_TDM_CFG_REG2_RXS_MASK: u8 = 0x03;
const TAS2770_TDM_CFG_REG2_RXS_16BITS: u8 = 0x00;
const TAS2770_TDM_CFG_REG2_RXS_24BITS: u8 = 1 << 0;
const TAS2770_TDM_CFG_REG2_RXS_32BITS: u8 = 0x02;
const TAS2770_TDM_CFG_REG3: u8 = 0x0d;
const TAS2770_TDM_CFG_REG3_LEFT_SLOT_MASK: u8 = 0x0f;
const TAS2770_TDM_CFG_REG3_RIGHT_SLOT_MASK: u8 = 0xf0;
const TAS2770_TDM_CFG_REG3_RIGHT_SLOT_SHIFT: u8 = 4;
const TAS2770_TDM_CFG_REG4: u8 = 0x0e;
const TAS2770_TDM_CFG_REG4_TX_FILL: u8 = 1 << 4;
const TAS2770_TDM_CFG_REG5: u8 = 0x0f;
const TAS2770_TDM_CFG_REG5_VSNS_MASK: u8 = 1 << 6;
const TAS2770_TDM_CFG_REG5_VSNS_ENABLE: u8 = 1 << 6;
const TAS2770_TDM_CFG_REG5_SLOT_MASK: u8 = 0x3f;
const TAS2770_TDM_CFG_REG6: u8 = 0x10;
const TAS2770_TDM_CFG_REG6_ISNS_MASK: u8 = 1 << 6;
const TAS2770_TDM_CFG_REG6_ISNS_ENABLE: u8 = 1 << 6;
const TAS2770_TDM_CFG_REG6_SLOT_MASK: u8 = 0x3f;
const TAS2770_DIN_PD: u8 = 0x31;
const TAS2770_DIN_PD_SDOUT: u8 = 1 << 7;

static TAS5770L_CODECS: Mutex<Vec<Arc<Tas5770l>>> = Mutex::new(Vec::new());

struct TasGpio {
    controller: Arc<dyn GpioController>,
    pin: u32,
    active_low: bool,
}

impl TasGpio {
    fn set_output(&self, active: bool) {
        self.controller
            .set_direction_output(self.pin, self.physical_value(active));
    }

    fn set(&self, active: bool) {
        self.controller
            .set_value(self.pin, self.physical_value(active));
    }

    fn physical_value(&self, active: bool) -> bool {
        if self.active_low { !active } else { active }
    }
}

struct TasFixedSupply {
    gpio: Option<TasGpio>,
    startup_delay_us: u64,
}

impl TasFixedSupply {
    fn enable(&self) {
        if let Some(gpio) = &self.gpio {
            gpio.set_output(true);
        }
        if self.startup_delay_us != 0 {
            udelay(self.startup_delay_us);
        }
    }
}

struct Tas5770l {
    bus: Arc<dyn I2cBus>,
    address: I2cAddress,
    bus_phandle: u32,
    sdz_supply: Option<TasFixedSupply>,
    shutdown_gpio: Option<TasGpio>,
    reset_gpio: Option<TasGpio>,
    i_sense_slot: Option<u8>,
    v_sense_slot: Option<u8>,
    sdout_pull_down: bool,
    sdout_zero_fill: bool,
    powered: Mutex<bool>,
    unmuted: Mutex<bool>,
}

impl Tas5770l {
    fn new(
        bus: Arc<dyn I2cBus>,
        address: I2cAddress,
        bus_phandle: u32,
        sdz_supply: Option<TasFixedSupply>,
        shutdown_gpio: Option<TasGpio>,
        reset_gpio: Option<TasGpio>,
        i_sense_slot: Option<u8>,
        v_sense_slot: Option<u8>,
        sdout_pull_down: bool,
        sdout_zero_fill: bool,
    ) -> Self {
        Self {
            bus,
            address,
            bus_phandle,
            sdz_supply,
            shutdown_gpio,
            reset_gpio,
            i_sense_slot,
            v_sense_slot,
            sdout_pull_down,
            sdout_zero_fill,
            powered: Mutex::new(false),
            unmuted: Mutex::new(false),
        }
    }

    fn write_register(&self, register: u8, value: u8) -> Result<(), I2cError> {
        let mut messages = alloc::vec![I2cMessage::write(self.address, &[register, value], true)];
        self.bus.transfer(&mut messages)
    }

    fn read_register(&self, register: u8) -> Result<u8, I2cError> {
        let mut messages = alloc::vec![
            I2cMessage::write(self.address, &[register], false),
            I2cMessage::read(self.address, 1, true),
        ];
        self.bus.transfer(&mut messages)?;
        Ok(messages[1].data[0])
    }

    fn update_bits(&self, register: u8, mask: u8, value: u8) -> Result<(), I2cError> {
        let current = self.read_register(register)?;
        self.write_register(register, (current & !mask) | (value & mask))
    }

    fn power_gpio(&self, active: bool) {
        if let Some(gpio) = &self.shutdown_gpio {
            gpio.set_output(active);
        }
    }

    fn hardware_reset(&self) {
        if let Some(gpio) = &self.reset_gpio {
            gpio.set_output(false);
            udelay(20_000);
            gpio.set(true);
            udelay(2_000);
        }
    }

    fn software_reset(&self) -> Result<(), I2cError> {
        self.write_register(TAS2770_SW_RST, TAS2770_RST)?;
        udelay(2_000);
        Ok(())
    }

    fn update_power_ctrl(&self) -> Result<(), I2cError> {
        let powered = *self.powered.lock();
        let unmuted = *self.unmuted.lock();
        let value = if powered {
            if unmuted {
                TAS2770_PWR_CTRL_ACTIVE
            } else {
                TAS2770_PWR_CTRL_MUTE
            }
        } else {
            TAS2770_PWR_CTRL_SHUTDOWN
        };
        self.update_bits(TAS2770_PWR_CTRL, TAS2770_PWR_CTRL_MASK, value)
    }

    fn set_powered(&self, powered: bool) -> Result<(), I2cError> {
        *self.powered.lock() = powered;
        self.update_power_ctrl()
    }

    fn set_muted(&self, muted: bool) -> Result<(), I2cError> {
        *self.unmuted.lock() = !muted;
        self.update_power_ctrl()
    }

    fn configure_i2s_ib_if(&self) -> Result<(), I2cError> {
        self.update_bits(
            TAS2770_TDM_CFG_REG1,
            TAS2770_TDM_CFG_REG1_RX_EDGE_MASK,
            TAS2770_TDM_CFG_REG1_RX_FALLING,
        )?;
        self.update_bits(
            TAS2770_TDM_CFG_REG1,
            TAS2770_TDM_CFG_REG1_START_SLOT_MASK,
            1 << TAS2770_TDM_CFG_REG1_START_SLOT_SHIFT,
        )?;
        self.update_bits(
            TAS2770_TDM_CFG_REG0,
            TAS2770_TDM_CFG_REG0_FPOL_MASK,
            TAS2770_TDM_CFG_REG0_FPOL_FALLING,
        )
    }

    fn configure_bitwidth(&self, format: u32) -> Result<(), I2cError> {
        let (rxw, rxs) = match format {
            AUDIO_PCM_FORMAT_S16LE => (
                TAS2770_TDM_CFG_REG2_RXW_16BITS,
                TAS2770_TDM_CFG_REG2_RXS_16BITS,
            ),
            AUDIO_PCM_FORMAT_S24LE3 => (
                TAS2770_TDM_CFG_REG2_RXW_24BITS,
                TAS2770_TDM_CFG_REG2_RXS_24BITS,
            ),
            AUDIO_PCM_FORMAT_S32LE => (
                TAS2770_TDM_CFG_REG2_RXW_32BITS,
                TAS2770_TDM_CFG_REG2_RXS_32BITS,
            ),
            _ => return Err(I2cError::InvalidArg),
        };
        self.update_bits(TAS2770_TDM_CFG_REG2, TAS2770_TDM_CFG_REG2_RXW_MASK, rxw)?;
        self.update_bits(TAS2770_TDM_CFG_REG2, TAS2770_TDM_CFG_REG2_RXS_MASK, rxs)
    }

    fn configure_sample_rate(&self, rate: u32) -> Result<(), I2cError> {
        let value = match rate {
            44_100 => TAS2770_TDM_CFG_REG0_SMP_44_1KHZ | TAS2770_TDM_CFG_REG0_RATE_44_1_48KHZ,
            48_000 => TAS2770_TDM_CFG_REG0_SMP_48KHZ | TAS2770_TDM_CFG_REG0_RATE_44_1_48KHZ,
            88_200 => TAS2770_TDM_CFG_REG0_SMP_44_1KHZ | TAS2770_TDM_CFG_REG0_RATE_88_2_96KHZ,
            96_000 => TAS2770_TDM_CFG_REG0_SMP_48KHZ | TAS2770_TDM_CFG_REG0_RATE_88_2_96KHZ,
            176_400 => TAS2770_TDM_CFG_REG0_SMP_44_1KHZ | TAS2770_TDM_CFG_REG0_RATE_176_4_192KHZ,
            192_000 => TAS2770_TDM_CFG_REG0_SMP_48KHZ | TAS2770_TDM_CFG_REG0_RATE_176_4_192KHZ,
            _ => return Err(I2cError::InvalidArg),
        };
        self.update_bits(
            TAS2770_TDM_CFG_REG0,
            TAS2770_TDM_CFG_REG0_SMP_MASK | TAS2770_TDM_CFG_REG0_RATE_MASK,
            value,
        )
    }

    fn configure_tdm_slot(
        &self,
        tx_mask: u32,
        slots: usize,
        slot_width: usize,
    ) -> Result<(), I2cError> {
        if tx_mask == 0 || slots == 0 {
            return Err(I2cError::InvalidArg);
        }
        let left_slot = tx_mask.trailing_zeros() as usize;
        let mut remaining = tx_mask & !(1u32 << left_slot);
        let right_slot = if remaining == 0 {
            left_slot
        } else {
            let slot = remaining.trailing_zeros() as usize;
            remaining &= !(1u32 << slot);
            slot
        };
        if remaining != 0
            || left_slot >= slots
            || right_slot >= slots
            || left_slot > 0x0f
            || right_slot > 0x0f
        {
            return Err(I2cError::InvalidArg);
        }

        let slot_width_value = match slot_width {
            16 => TAS2770_TDM_CFG_REG2_RXS_16BITS,
            24 => TAS2770_TDM_CFG_REG2_RXS_24BITS,
            32 => TAS2770_TDM_CFG_REG2_RXS_32BITS,
            _ => return Err(I2cError::InvalidArg),
        };
        self.update_bits(
            TAS2770_TDM_CFG_REG3,
            TAS2770_TDM_CFG_REG3_LEFT_SLOT_MASK,
            left_slot as u8,
        )?;
        self.update_bits(
            TAS2770_TDM_CFG_REG3,
            TAS2770_TDM_CFG_REG3_RIGHT_SLOT_MASK,
            (right_slot as u8) << TAS2770_TDM_CFG_REG3_RIGHT_SLOT_SHIFT,
        )?;
        self.update_bits(
            TAS2770_TDM_CFG_REG2,
            TAS2770_TDM_CFG_REG2_RXS_MASK,
            slot_width_value,
        )
    }

    fn configure_ivsense_transmit(&self) -> Result<(), I2cError> {
        let Some(i_slot) = self.i_sense_slot else {
            return Ok(());
        };
        let Some(v_slot) = self.v_sense_slot else {
            return Ok(());
        };
        if i_slot & !TAS2770_TDM_CFG_REG6_SLOT_MASK != 0
            || v_slot & !TAS2770_TDM_CFG_REG5_SLOT_MASK != 0
        {
            return Err(I2cError::InvalidArg);
        }

        self.update_bits(
            TAS2770_TDM_CFG_REG5,
            TAS2770_TDM_CFG_REG5_VSNS_MASK | TAS2770_TDM_CFG_REG5_SLOT_MASK,
            TAS2770_TDM_CFG_REG5_VSNS_ENABLE | v_slot,
        )?;
        self.update_bits(
            TAS2770_TDM_CFG_REG6,
            TAS2770_TDM_CFG_REG6_ISNS_MASK | TAS2770_TDM_CFG_REG6_SLOT_MASK,
            TAS2770_TDM_CFG_REG6_ISNS_ENABLE | i_slot,
        )
    }

    fn configure_playback(
        &self,
        format: u32,
        rate: u32,
        tx_mask: u32,
        slots: usize,
        slot_width: usize,
    ) -> Result<(), I2cError> {
        self.configure_i2s_ib_if()?;
        self.configure_bitwidth(format)?;
        self.configure_sample_rate(rate)?;
        self.configure_tdm_slot(tx_mask, slots, slot_width)?;
        self.configure_ivsense_transmit()?;
        self.update_bits(
            TAS2770_TDM_CFG_REG4,
            TAS2770_TDM_CFG_REG4_TX_FILL,
            if self.sdout_zero_fill {
                0
            } else {
                TAS2770_TDM_CFG_REG4_TX_FILL
            },
        )?;
        self.update_bits(
            TAS2770_DIN_PD,
            TAS2770_DIN_PD_SDOUT,
            if self.sdout_pull_down {
                TAS2770_DIN_PD_SDOUT
            } else {
                0
            },
        )
    }

    fn initialize(&self) -> Result<(), I2cError> {
        if let Some(supply) = &self.sdz_supply {
            supply.enable();
        }
        self.power_gpio(true);
        udelay(2_000);
        self.hardware_reset();
        self.software_reset()?;
        self.configure_playback(
            AUDIO_PCM_FORMAT_S16LE,
            TAS5770L_DEFAULT_RATE,
            TAS5770L_DEFAULT_TX_MASK,
            TAS5770L_DEFAULT_SLOTS,
            TAS5770L_DEFAULT_SLOT_WIDTH,
        )?;
        self.set_powered(true)?;
        self.set_muted(true)
    }
}

impl AudioCodec for Tas5770l {
    fn configure_playback(
        &self,
        params: &AudioPcmParams,
        tx_mask: u32,
        slots: usize,
        slot_width: usize,
    ) -> Result<(), &'static str> {
        self.configure_playback(params.format, params.rate, tx_mask, slots, slot_width)
            .map_err(|_| "tas5770l: failed to configure playback")
    }

    fn set_playback_muted(&self, muted: bool) -> Result<(), &'static str> {
        self.set_muted(muted)
            .map_err(|_| "tas5770l: failed to change mute state")
    }

    fn set_playback_powered(&self, powered: bool) -> Result<(), &'static str> {
        self.set_powered(powered)
            .map_err(|_| "tas5770l: failed to change power state")
    }
}

fn read_i2c_address(device: &PlatformDeviceInfo) -> Result<I2cAddress, &'static str> {
    let address = device
        .property("reg")
        .and_then(|property| property.as_usize())
        .ok_or("tas5770l: missing I2C address")?;
    if address > TAS5770L_MAX_7BIT_ADDRESS {
        return Err("tas5770l: unsupported I2C address");
    }

    Ok(I2cAddress::SevenBit(address as u8))
}

fn read_sound_dai_cells(device: &PlatformDeviceInfo) -> Result<usize, &'static str> {
    let cells = device
        .property("#sound-dai-cells")
        .and_then(|property| property.as_usize())
        .unwrap_or(TAS5770L_SOUND_DAI_CELLS);
    if cells != TAS5770L_SOUND_DAI_CELLS {
        return Err("tas5770l: unsupported #sound-dai-cells");
    }

    Ok(cells)
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|value| value as u32)
        .ok_or("tas5770l: missing phandle")
}

fn read_optional_u8_property(
    device: &PlatformDeviceInfo,
    name: &str,
) -> Result<Option<u8>, &'static str> {
    let Some(value) = device
        .property(name)
        .and_then(|property| property.as_usize())
    else {
        return Ok(None);
    };
    if value > u8::MAX as usize {
        return Err("tas5770l: property value is too large");
    }

    Ok(Some(value as u8))
}

fn read_be_u32_cells(value: &[u8]) -> impl Iterator<Item = u32> + '_ {
    value
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn read_be_u32(value: &[u8]) -> Option<u32> {
    if value.len() < 4 {
        return None;
    }
    Some(u32::from_be_bytes([value[0], value[1], value[2], value[3]]))
}

fn resolve_gpio(device: &PlatformDeviceInfo, name: &str) -> Result<Option<TasGpio>, &'static str> {
    let Some(property) = device.property(name) else {
        return Ok(None);
    };
    let mut cells = read_be_u32_cells(property.value());
    let phandle = cells.next().ok_or("tas5770l: malformed GPIO property")?;
    let pin = cells.next().ok_or("tas5770l: malformed GPIO property")?;
    let flags = cells.next().unwrap_or(0);
    match DeviceManager::get_manager().get_gpio_controller(phandle) {
        Some(controller) => Ok(Some(TasGpio {
            controller,
            pin,
            active_low: flags & 1 != 0,
        })),
        None => {
            early_println!(
                "[tas5770l] GPIO controller phandle {:#x} for {} is not ready, deferring",
                phandle,
                name
            );
            probe_defer()
        }
    }
}

fn resolve_fixed_supply(
    device: &PlatformDeviceInfo,
    name: &str,
) -> Result<Option<TasFixedSupply>, &'static str> {
    let Some(property) = device.property(name) else {
        return Ok(None);
    };
    let phandle = read_be_u32(property.value()).ok_or("tas5770l: malformed supply property")?;
    if phandle == 0 {
        return Ok(None);
    }

    let fdt = FdtManager::get_manager()
        .get_fdt()
        .ok_or("tas5770l: FDT is not available")?;
    let regulator = {
        let mut stack = Vec::new();
        stack.push(fdt.find_node("/").ok_or("tas5770l: missing FDT root")?);
        let mut found = None;

        while let Some(node) = stack.pop() {
            let node_phandle = node
                .property("phandle")
                .or_else(|| node.property("linux,phandle"))
                .and_then(|property| read_be_u32(property.value));
            if node_phandle == Some(phandle) {
                found = Some(node);
                break;
            }

            for child in node.children() {
                stack.push(child);
            }
        }

        found.ok_or("tas5770l: supply node not found")?
    };

    let is_fixed_regulator = regulator
        .compatible()
        .is_some_and(|compatible| compatible.all().any(|entry| entry == "regulator-fixed"));
    if !is_fixed_regulator {
        return Err("tas5770l: unsupported supply type");
    }

    let startup_delay_us = regulator
        .property("startup-delay-us")
        .and_then(|property| read_be_u32(property.value))
        .unwrap_or(0) as u64;
    let gpio_property = regulator
        .property("gpios")
        .or_else(|| regulator.property("gpio"));
    let gpio = match gpio_property {
        Some(property) => {
            let mut cells = read_be_u32_cells(property.value);
            let gpio_phandle = cells
                .next()
                .ok_or("tas5770l: malformed supply GPIO property")?;
            let pin = cells
                .next()
                .ok_or("tas5770l: malformed supply GPIO property")?;
            let flags = cells.next().unwrap_or(0);
            let active_low = if flags & 1 != 0 {
                true
            } else {
                regulator.property("enable-active-high").is_none()
            };

            match DeviceManager::get_manager().get_gpio_controller(gpio_phandle) {
                Some(controller) => Some(TasGpio {
                    controller,
                    pin,
                    active_low,
                }),
                None => {
                    early_println!(
                        "[tas5770l] GPIO controller phandle {:#x} for {} regulator is not ready, deferring",
                        gpio_phandle,
                        name
                    );
                    return probe_defer();
                }
            }
        }
        None => None,
    };

    early_println!(
        "[tas5770l] resolved {} fixed regulator phandle={:#x}, gpio={}, startup-delay-us={}",
        name,
        phandle,
        gpio.is_some(),
        startup_delay_us
    );

    Ok(Some(TasFixedSupply {
        gpio,
        startup_delay_us,
    }))
}

fn resolve_i2c_bus(device: &PlatformDeviceInfo) -> Result<(u32, Arc<dyn I2cBus>), &'static str> {
    let bus_phandle = device
        .parent_phandle()
        .ok_or("tas5770l: missing parent I2C bus")?;
    match DeviceManager::get_manager().get_i2c_bus(bus_phandle) {
        Some(bus) => Ok((bus_phandle, bus)),
        None => {
            early_println!(
                "[tas5770l] I2C bus phandle {:#x} is not ready, deferring",
                bus_phandle
            );
            probe_defer()
        }
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let (bus_phandle, bus) = resolve_i2c_bus(device)?;
    let phandle = read_phandle(device)?;
    let address = read_i2c_address(device)?;
    let _sound_dai_cells = read_sound_dai_cells(device)?;
    let sdz_supply = resolve_fixed_supply(device, "SDZ-supply")?;
    let shutdown_gpio = resolve_gpio(device, "shutdown-gpios")?;
    let reset_gpio = resolve_gpio(device, "reset-gpios")?;
    let i_sense_slot = read_optional_u8_property(device, "ti,imon-slot-no")?;
    let v_sense_slot = read_optional_u8_property(device, "ti,vmon-slot-no")?;
    let sdout_pull_down = device.property("ti,sdout-pull-down").is_some();
    let sdout_zero_fill = device.property("ti,sdout-zero-fill").is_some();
    let has_shutdown_gpio = shutdown_gpio.is_some();
    let has_reset_gpio = reset_gpio.is_some();
    let codec = Arc::new(Tas5770l::new(
        bus,
        address,
        bus_phandle,
        sdz_supply,
        shutdown_gpio,
        reset_gpio,
        i_sense_slot,
        v_sense_slot,
        sdout_pull_down,
        sdout_zero_fill,
    ));
    if let Err(error) = codec.initialize() {
        early_println!(
            "[tas5770l] initialization failed for phandle={:#x}, addr={:#x}: {:?}",
            phandle,
            address.raw(),
            error
        );
        return Err("tas5770l: codec initialization failed");
    }
    let audio_codec: Arc<dyn AudioCodec> = codec.clone();
    DeviceManager::get_manager().register_audio_codec(phandle, audio_codec);
    TAS5770L_CODECS.lock().push(codec);

    early_println!(
        "[tas5770l] registered {} at phandle={:#x}, bus-phandle={:#x}, addr={:#x}, sdz-supply={}, shutdown-gpio={}, reset-gpio={}, i-sense-slot={:?}, v-sense-slot={:?}, sdout-pull-down={}, sdout-zero-fill={}",
        device.name(),
        phandle,
        bus_phandle,
        address.raw(),
        device.property("SDZ-supply").is_some(),
        has_shutdown_gpio,
        has_reset_gpio,
        i_sense_slot,
        v_sense_slot,
        sdout_pull_down,
        sdout_zero_fill
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "tas5770l",
        probe_fn,
        remove_fn,
        alloc::vec!["ti,tas5770l", "ti,tas2770"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_TAS5770L_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
