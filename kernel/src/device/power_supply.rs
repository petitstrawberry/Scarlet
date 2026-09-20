//! Read-only battery and external-supply telemetry, independent of platform.
//! Provider I/O is performed after releasing the registry lock. IDs remain
//! stable until reboot; an unreadable supply still retains its identity.

use crate::sync::IrqSpinLock;
use alloc::{sync::Arc, vec::Vec};
pub use scarlet_abi::power_supply::{ChargeState, PowerSupplyState, SupplyKind};
use scarlet_abi::power_supply::{POWER_SUPPLY_MAX_DEVICES, PowerSupplySnapshot};

pub trait PowerSupply: Send + Sync {
    fn name(&self) -> &'static str;
    fn kind(&self) -> SupplyKind;
    /// Bounded, read-only hardware observation. Unsupported measurements are
    /// None. Drivers must not change charger policy or battery calibration.
    fn read(&self) -> Result<PowerSupplyState, &'static str>;
}

static SUPPLIES: IrqSpinLock<Vec<Arc<dyn PowerSupply>>> = IrqSpinLock::new(Vec::new());

pub fn register(supply: Arc<dyn PowerSupply>) -> Result<u32, &'static str> {
    let name = supply.name();
    if name.is_empty() || name.len() >= 32 || !name.is_ascii() || name.contains('\0') {
        return Err("invalid power supply name");
    }
    let mut supplies = SUPPLIES.lock();
    if supplies.len() >= POWER_SUPPLY_MAX_DEVICES {
        return Err("too many power supplies");
    }
    if supplies.iter().any(|s| s.name() == name) {
        return Err("power supply already registered");
    }
    let id = supplies.len() as u32;
    supplies.push(supply);
    Ok(id)
}

pub fn count() -> usize {
    SUPPLIES.lock().len()
}

pub fn snapshot(id: u32) -> Result<PowerSupplySnapshot, &'static str> {
    let supply = { SUPPLIES.lock().get(id as usize).cloned() }.ok_or("unknown power supply ID")?;
    let result = supply.read();
    let mut snapshot = PowerSupplySnapshot {
        id,
        kind: supply.kind(),
        sampled_at_ns: crate::time::current_time_ns(),
        read_failed: result.is_err(),
        state: result.unwrap_or_default(),
        ..Default::default()
    };
    snapshot.name[..supply.name().len()].copy_from_slice(supply.name().as_bytes());
    Ok(snapshot)
}
