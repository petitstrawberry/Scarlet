//! Logical CPU slots are independent of firmware hart identifiers.

use crate::environment::MAX_NUM_CPUS;
use crate::sync::Once;

struct CpuHarts {
    harts: [Option<usize>; MAX_NUM_CPUS],
    len: usize,
}

impl CpuHarts {
    fn new() -> Self {
        Self {
            harts: [None; MAX_NUM_CPUS],
            len: 0,
        }
    }

    fn insert(&mut self, hart_id: usize) -> Result<(), &'static str> {
        if self.harts[..self.len].contains(&Some(hart_id)) {
            return Err("duplicate hart ID");
        }
        let slot = self
            .harts
            .get_mut(self.len)
            .ok_or("CPU inventory exceeds kernel capacity")?;
        *slot = Some(hart_id);
        self.len += 1;
        Ok(())
    }

    fn logical_id(&self, hart_id: usize) -> Option<usize> {
        self.harts[..self.len]
            .iter()
            .position(|hart| *hart == Some(hart_id))
    }
}

static CPU_HARTS: Once<CpuHarts> = Once::new();

pub(crate) fn init_from_fdt(boot_hart: usize) -> Result<(), &'static str> {
    let fdt = crate::device::fdt::FdtManager::get_manager()
        .get_fdt()
        .ok_or("missing CPU FDT")?;
    let cpus = fdt.find_node("/cpus").ok_or("missing /cpus node")?;
    let mut harts = CpuHarts::new();
    // The boot adapter assigns slot zero to the hart whose entry stack uses
    // slot zero. Physical IDs may be sparse, reordered, or exceed the hart-mask bit count.
    harts.insert(boot_hart)?;
    let mut saw_boot = false;
    for node in cpus.children() {
        if node.property("device_type").and_then(|p| p.as_str()) != Some("cpu") {
            continue;
        }
        if node
            .property("status")
            .is_some_and(|p| !matches!(p.as_str(), Some("okay" | "ok")))
        {
            continue;
        }
        let reg = node
            .raw_reg()
            .and_then(|mut regs| regs.next())
            .ok_or("CPU has no hart reg")?;
        let hart = crate::device::fdt::decode_address(reg.address).ok_or("invalid hart ID")?;
        let hart = usize::try_from(hart).map_err(|_| "hart ID exceeds SBI argument width")?;
        if hart == boot_hart {
            if saw_boot {
                return Err("duplicate boot hart ID");
            }
            saw_boot = true;
        } else {
            harts.insert(hart)?;
        }
    }
    if !saw_boot {
        return Err("boot hart is absent from enabled CPU inventory");
    }
    assert!(CPU_HARTS.get().is_none(), "CPU inventory is boot-immutable");
    CPU_HARTS.call_once(|| harts);
    Ok(())
}

/// Publish an immutable firmware inventory, assigning the BSP logical slot 0.
#[cfg(feature = "limine")]
pub(crate) fn init_harts(
    boot_hart: usize,
    inventory: impl Iterator<Item = usize>,
) -> Result<(), &'static str> {
    let mut harts = CpuHarts::new();
    harts.insert(boot_hart)?;
    let mut saw_boot = false;
    for hart in inventory {
        if hart == boot_hart {
            if saw_boot {
                return Err("duplicate boot hart ID");
            }
            saw_boot = true;
        } else {
            harts.insert(hart)?;
        }
    }
    if !saw_boot {
        return Err("boot hart missing from firmware inventory");
    }
    assert!(CPU_HARTS.get().is_none(), "CPU inventory is boot-immutable");
    CPU_HARTS.call_once(|| harts);
    Ok(())
}

pub(crate) fn count() -> usize {
    CPU_HARTS.get().map_or(0, |harts| harts.len)
}

pub(crate) fn hart_id(cpu_id: usize) -> Option<usize> {
    CPU_HARTS.get()?.harts.get(cpu_id).copied().flatten()
}

pub(crate) fn logical_id(hart_id: usize) -> Option<usize> {
    CPU_HARTS.get()?.logical_id(hart_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn sparse_harts_use_bounded_logical_slots() {
        let mut harts = CpuHarts::new();
        harts.insert(usize::MAX).unwrap();
        harts.insert(17).unwrap();
        harts.insert(2).unwrap();
        assert_eq!(harts.logical_id(usize::MAX), Some(0));
        assert_eq!(harts.logical_id(17), Some(1));
        assert_eq!(harts.logical_id(2), Some(2));
        assert_eq!(harts.logical_id(0), None);
        assert!(harts.insert(17).is_err());
        for hart in 100..100 + MAX_NUM_CPUS - 3 {
            harts.insert(hart).unwrap();
        }
        assert!(harts.insert(999).is_err());
    }
}
