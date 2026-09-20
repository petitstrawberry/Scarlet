//! Control-only read-only telemetry endpoint, `/dev/power_supply`.
use crate::{
    device::{Device, DeviceType, char::CharDevice, manager::DeviceManager, power_supply},
    library::std::usercopy::{copy_from_user, copy_to_user},
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{SelectWaitOutcome, Selectable},
    },
};
use alloc::{string::String, sync::Arc};
use core::any::Any;
use scarlet_abi::power_supply::{
    SCTL_POWER_SUPPLY_COUNT, SCTL_POWER_SUPPLY_SNAPSHOT, snapshot_request_id,
};

struct PowerSupplyDevice;
impl Device for PowerSupplyDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "power_supply"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}
impl CharDevice for PowerSupplyDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }
    fn write_byte(&self, _: u8) -> Result<(), &'static str> {
        Err("read-only power supply")
    }
    fn write(&self, _: &[u8]) -> Result<usize, &'static str> {
        Err("read-only power supply")
    }
    fn can_read(&self) -> bool {
        false
    }
    fn can_write(&self) -> bool {
        false
    }
}
impl ControlOps for PowerSupplyDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            SCTL_POWER_SUPPLY_COUNT => Ok(power_supply::count() as i32),
            SCTL_POWER_SUPPLY_SNAPSHOT => {
                let task = crate::task::mytask().ok_or("no power supply query task")?;
                let mut header = [0; 12];
                copy_from_user(&task, arg, &mut header)
                    .map_err(|_| "invalid power supply request pointer")?;
                let id =
                    snapshot_request_id(&header).ok_or("invalid power supply request header")?;
                let bytes = power_supply::snapshot(id)?.encode();
                copy_to_user(&task, arg, &bytes)
                    .map_err(|_| "invalid power supply output pointer")?;
                Ok(0)
            }
            _ => Err("unsupported power supply control"),
        }
    }
}
impl MemoryMappingOps for PowerSupplyDevice {
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("power supply mapping unsupported")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}
impl Selectable for PowerSupplyDevice {
    fn wait_until_ready(
        &self,
        _: crate::object::capability::selectable::ReadyInterest,
        _: &mut crate::arch::Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}
fn register() {
    DeviceManager::get_manager()
        .register_device_with_name(String::from("power_supply"), Arc::new(PowerSupplyDevice));
}
crate::driver_initcall!(register);
