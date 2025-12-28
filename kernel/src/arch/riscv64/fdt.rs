use crate::device::fdt::FdtManager;

/// Read the RISC-V timebase frequency (Hz) from the device tree.
///
/// The standard location is the `/cpus` node property `timebase-frequency`.
/// Returns `None` if FDT is unavailable or the property is missing/invalid.
pub fn timebase_frequency_hz_from_fdt() -> Option<u64> {
    let fdt = FdtManager::get_manager().get_fdt()?;
    let cpus = fdt.find_node("/cpus")?;

    let prop = cpus
        .property("timebase-frequency")
        .or_else(|| cpus.property("riscv,timebase-frequency"))?;

    match prop.value.len() {
        4 => Some(
            u32::from_be_bytes([prop.value[0], prop.value[1], prop.value[2], prop.value[3]]) as u64,
        ),
        8 => Some(u64::from_be_bytes([
            prop.value[0],
            prop.value[1],
            prop.value[2],
            prop.value[3],
            prop.value[4],
            prop.value[5],
            prop.value[6],
            prop.value[7],
        ])),
        _ => None,
    }
}
