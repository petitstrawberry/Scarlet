#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use scarlet::{
    device::{
        DeviceInfo,
        i2c::{I2cAddress, I2cBus, I2cError, I2cMessage},
        manager::{DeviceManager, DriverPriority, probe_defer},
        platform::{PlatformDeviceDriver, PlatformDeviceInfo},
    },
    early_println,
    sync::Mutex,
    time::udelay,
};

const CD321X_MAX_7BIT_ADDRESS: usize = 0x7f;
const TPS_MAX_LEN: usize = 64;
const I2C_SETTLE_US: u64 = 500;

const TPS_REG_VID: u8 = 0x00;
const TPS_REG_MODE: u8 = 0x03;
const TPS_REG_STATUS: u8 = 0x1a;
const TPS_REG_POWER_STATUS: u8 = 0x3f;
const TPS_REG_DATA_STATUS: u8 = 0x5f;

struct AppleCd321x {
    bus: Arc<dyn I2cBus>,
    address: I2cAddress,
    bus_phandle: u32,
}

#[derive(Debug, Clone, Copy)]
struct Cd321xSnapshot {
    vendor_id: u32,
    mode: [u8; 4],
    status: u32,
    power_status: u32,
    data_status: u32,
}

impl AppleCd321x {
    fn new(bus: Arc<dyn I2cBus>, address: I2cAddress, bus_phandle: u32) -> Self {
        Self {
            bus,
            address,
            bus_phandle,
        }
    }

    fn read_exact<const N: usize>(&self, register: u8) -> Result<[u8; N], I2cError> {
        if N > TPS_MAX_LEN {
            return Err(I2cError::InvalidArg);
        }

        let mut messages = alloc::vec![
            I2cMessage::write(self.address, &[register], false),
            I2cMessage::read(self.address, N + 1, true),
        ];
        self.bus.transfer(&mut messages)?;
        udelay(I2C_SETTLE_US);

        let data = messages[1].data.as_slice();
        let declared_len = *data.first().ok_or(I2cError::BusError)?;
        if usize::from(declared_len) < N {
            return Err(I2cError::BusError);
        }

        let mut out = [0u8; N];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = *data.get(index + 1).ok_or(I2cError::BusError)?;
        }
        Ok(out)
    }

    fn read_u32(&self, register: u8) -> Result<u32, I2cError> {
        Ok(u32::from_le_bytes(self.read_exact::<4>(register)?))
    }

    fn snapshot(&self) -> Result<Cd321xSnapshot, I2cError> {
        Ok(Cd321xSnapshot {
            vendor_id: self.read_u32(TPS_REG_VID)?,
            mode: self.read_exact::<4>(TPS_REG_MODE)?,
            status: self.read_u32(TPS_REG_STATUS)?,
            power_status: self.read_u32(TPS_REG_POWER_STATUS)?,
            data_status: self.read_u32(TPS_REG_DATA_STATUS)?,
        })
    }
}

fn printable_ascii(byte: u8) -> char {
    if byte.is_ascii_graphic() || byte == b' ' {
        char::from(byte)
    } else {
        '.'
    }
}

fn read_i2c_address(device: &PlatformDeviceInfo) -> Result<I2cAddress, &'static str> {
    let address = device
        .property("reg")
        .and_then(|property| property.as_usize())
        .ok_or("apple-cd321x: missing I2C address")?;
    if address > CD321X_MAX_7BIT_ADDRESS {
        return Err("apple-cd321x: unsupported I2C address");
    }

    Ok(I2cAddress::SevenBit(
        u8::try_from(address).map_err(|_| "apple-cd321x: unsupported I2C address")?,
    ))
}

fn resolve_i2c_bus(device: &PlatformDeviceInfo) -> Result<(u32, Arc<dyn I2cBus>), &'static str> {
    let bus_phandle = device
        .parent_phandle()
        .ok_or("apple-cd321x: missing parent I2C bus")?;
    match DeviceManager::get_manager().get_i2c_bus(bus_phandle) {
        Some(bus) => Ok((bus_phandle, bus)),
        None => {
            early_println!(
                "[apple-cd321x] I2C bus phandle {:#x} is not ready, deferring",
                bus_phandle
            );
            probe_defer()
        }
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let (bus_phandle, bus) = resolve_i2c_bus(device)?;
    let address = read_i2c_address(device)?;
    let controller = Arc::new(AppleCd321x::new(bus, address, bus_phandle));
    let snapshot = controller.snapshot().map_err(|_| {
        early_println!(
            "[apple-cd321x] failed to read status for {} bus-phandle={:#x} addr={:#x}",
            device.name(),
            bus_phandle,
            address.raw(),
        );
        "apple-cd321x: failed to read status"
    })?;

    early_println!(
        "[apple-cd321x] registered {} bus-phandle={:#x} addr={:#x} vid=0x{:08x} mode={}{}{}{} status=0x{:08x} power=0x{:08x} data=0x{:08x}",
        device.name(),
        controller.bus_phandle,
        controller.address.raw(),
        snapshot.vendor_id,
        printable_ascii(snapshot.mode[0]),
        printable_ascii(snapshot.mode[1]),
        printable_ascii(snapshot.mode[2]),
        printable_ascii(snapshot.mode[3]),
        snapshot.status,
        snapshot.power_status,
        snapshot.data_status,
    );

    APPLE_CD321X.lock().push(controller);
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-cd321x",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,cd321x"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

static APPLE_CD321X: Mutex<Vec<Arc<AppleCd321x>>> = Mutex::new(Vec::new());

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_CD321X_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
