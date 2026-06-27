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
        i2c::{I2cAddress, I2cBus, I2cError, I2cMessage},
        manager::{DeviceManager, DriverPriority, probe_defer},
        platform::{PlatformDeviceDriver, PlatformDeviceInfo},
    },
    early_println,
};

const TAS5770L_MAX_7BIT_ADDRESS: usize = 0x7f;
const TAS5770L_SOUND_DAI_CELLS: usize = 0;

static TAS5770L_CODECS: Mutex<Vec<Arc<Tas5770l>>> = Mutex::new(Vec::new());

struct Tas5770l {
    bus: Arc<dyn I2cBus>,
    address: I2cAddress,
    bus_phandle: u32,
    has_sdz_supply: bool,
}

impl Tas5770l {
    fn new(
        bus: Arc<dyn I2cBus>,
        address: I2cAddress,
        bus_phandle: u32,
        has_sdz_supply: bool,
    ) -> Self {
        Self {
            bus,
            address,
            bus_phandle,
            has_sdz_supply,
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
    let address = read_i2c_address(device)?;
    let _sound_dai_cells = read_sound_dai_cells(device)?;
    let has_sdz_supply = device.property("SDZ-supply").is_some();
    let codec = Arc::new(Tas5770l::new(bus, address, bus_phandle, has_sdz_supply));
    TAS5770L_CODECS.lock().push(codec);

    early_println!(
        "[tas5770l] registered {} at bus-phandle={:#x}, addr={:#x}, sdz-supply={}",
        device.name(),
        bus_phandle,
        address.raw(),
        has_sdz_supply
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
