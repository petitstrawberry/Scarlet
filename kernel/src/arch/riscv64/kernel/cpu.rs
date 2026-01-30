//! RISC-V CPU information from device tree

use crate::device::fdt::FdtManager;
use crate::early_println;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Maximum CPU ID discovered from device tree
static MAX_CPU_ID: AtomicUsize = AtomicUsize::new(0);

/// Get CPU information from device tree
///
/// Returns the number of CPUs and the maximum CPU ID
pub fn get_cpu_info_from_fdt() -> Option<(usize, usize)> {
    let fdt = FdtManager::get_manager().get_fdt()?;
    let cpus_node = fdt.find_node("/cpus")?;

    let mut cpu_count = 0usize;
    let mut max_hart_id = 0usize;

    // Iterate through all CPU nodes
    for child in cpus_node.children() {
        // Only count nodes that start with "cpu@"
        if child.name.starts_with("cpu@") {
            early_println!(
                "[SMP] Found CPU node: {}, parent: {}",
                child.name,
                cpus_node.name
            );

            // Get the hart ID from the reg property
            if let Some(reg_prop) = child.property("reg") {
                if reg_prop.value.len() >= 8 {
                    // reg property is typically a 64-bit value for hart ID
                    let hart_id = u64::from_be_bytes([
                        reg_prop.value[0],
                        reg_prop.value[1],
                        reg_prop.value[2],
                        reg_prop.value[3],
                        reg_prop.value[4],
                        reg_prop.value[5],
                        reg_prop.value[6],
                        reg_prop.value[7],
                    ]) as usize;

                    early_println!("[SMP] CPU node {} has hart ID {}", child.name, hart_id);
                    cpu_count += 1;
                    if hart_id > max_hart_id {
                        max_hart_id = hart_id;
                    }
                }
            }
        }
    }

    early_println!(
        "[SMP] Device tree reports {} CPUs with max hart ID {}",
        cpu_count,
        max_hart_id
    );

    if cpu_count == 0 {
        None
    } else {
        Some((cpu_count, max_hart_id))
    }
}

/// Get the maximum CPU ID from device tree
pub fn get_max_cpu_id() -> usize {
    let cached = MAX_CPU_ID.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }

    if let Some((_, max_id)) = get_cpu_info_from_fdt() {
        MAX_CPU_ID.store(max_id, Ordering::Relaxed);
        max_id
    } else {
        // Default to NUM_OF_CPUS - 1 if device tree not available
        crate::environment::NUM_OF_CPUS - 1
    }
}

/// Get the boot CPU ID from device tree
pub fn get_boot_cpu_id() -> usize {
    // Try to get boot CPU ID from device tree
    let fdt = FdtManager::get_manager().get_fdt();
    if let Some(fdt) = fdt {
        if let Some(chosen) = fdt.find_node("/chosen") {
            if let Some(bootargs) = chosen.property("bootargs") {
                // Check if bootargs contains hartid parameter
                // This is a convention some bootloaders use
                let bootargs_str = core::str::from_utf8(bootargs.value).unwrap_or("");
                if let Some(hartid_str) = bootargs_str.split("hartid=").nth(1) {
                    if let Some(hartid) = hartid_str.split_whitespace().next() {
                        if let Ok(id) = hartid.parse::<usize>() {
                            early_println!("[SMP] Boot CPU ID from bootargs: {}", id);
                            return id;
                        }
                    }
                }
            }
        }
    }

    // Default to 0 if not specified
    0
}
