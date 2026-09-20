//! Typed, read-only power-supply discovery and telemetry.
use crate::handle::{Handle, HandleError, HandleResult};
pub use scarlet_abi::power_supply::{
    ChargeState, PowerSupplySnapshot, PowerSupplyState, SupplyKind,
};
use scarlet_abi::power_supply::{
    POWER_SUPPLY_MAX_DEVICES, SCTL_POWER_SUPPLY_COUNT, SCTL_POWER_SUPPLY_SNAPSHOT, snapshot_request,
};

#[derive(Debug)]
pub struct PowerSupplies {
    handle: Handle,
}
impl PowerSupplies {
    pub fn open() -> HandleResult<Self> {
        Ok(Self {
            handle: Handle::open("/dev/power_supply", 0)?,
        })
    }
    /// Supply IDs are 0..count and remain stable for the lifetime of the boot.
    pub fn count(&self) -> HandleResult<u32> {
        // SAFETY: COUNT takes no pointer or additional input.
        let count = unsafe { self.handle.control(SCTL_POWER_SUPPLY_COUNT, 0) }?;
        if count < 0 || count as usize > POWER_SUPPLY_MAX_DEVICES {
            return Err(HandleError::InvalidParameter);
        }
        Ok(count as u32)
    }
    pub fn snapshot(&self, id: u32) -> HandleResult<PowerSupplySnapshot> {
        let mut bytes = snapshot_request(id);
        // SAFETY: the fixed-size initialized request/output buffer is exclusive
        // and remains live throughout the synchronous control operation.
        unsafe {
            self.handle
                .control(SCTL_POWER_SUPPLY_SNAPSHOT, bytes.as_mut_ptr() as usize)
        }?;
        let snapshot = PowerSupplySnapshot::decode(&bytes).ok_or(HandleError::InvalidParameter)?;
        if snapshot.id != id {
            return Err(HandleError::InvalidParameter);
        }
        Ok(snapshot)
    }
}
