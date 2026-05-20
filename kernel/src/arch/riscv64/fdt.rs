use crate::device::fdt::FdtManager;

/// Read the RISC-V timebase frequency (Hz) from the device tree.
///
/// The standard location is the `/cpus` node property `timebase-frequency`.
/// Returns `None` if FDT is unavailable or the property is missing/invalid.
pub fn timebase_frequency_hz_from_fdt() -> Option<u64> {
    let fdt = FdtManager::get_manager().get_fdt()?;
    let cpus = fdt.find_node("/cpus")?;

    let prop = cpus.property("timebase-frequency")?;
    match prop.value.len() {
        4 => Some(u32::from_be_bytes(prop.value[0..4].try_into().unwrap()) as u64),
        8 => Some(u64::from_be_bytes(prop.value[0..8].try_into().unwrap())),
        _ => None,
    }
}

/// Check whether every enabled RISC-V CPU node reports an ISA extension.
///
/// # Arguments
///
/// * `extension` - Lowercase ISA extension name, such as `"sstc"`.
///
/// # Returns
///
/// `Some(true)` if every enabled CPU node with ISA data reports the extension,
/// `Some(false)` if at least one does not, or `None` when FDT or CPU ISA data is
/// unavailable.
pub fn all_cpus_have_isa_extension_from_fdt(extension: &str) -> Option<bool> {
    let fdt = FdtManager::get_manager().get_fdt()?;
    let cpus = fdt.find_node("/cpus")?;
    let extension = extension.as_bytes();

    let mut saw_cpu = false;
    let mut all_have_extension = true;

    for cpu in cpus.children() {
        if !is_enabled_cpu_node(&cpu) {
            continue;
        }

        let has_extension = if let Some(prop) = cpu.property("riscv,isa-extensions") {
            isa_extension_list_has_extension(prop.value, extension)
        } else if let Some(prop) = cpu.property("riscv,isa") {
            isa_string_has_extension(prop.value, extension)
        } else {
            return None;
        };

        saw_cpu = true;
        all_have_extension &= has_extension;
    }

    if saw_cpu {
        Some(all_have_extension)
    } else {
        None
    }
}

fn is_enabled_cpu_node(cpu: &fdt::node::FdtNode) -> bool {
    if let Some(dev_type) = cpu.property("device_type") {
        if bytes_to_cstr(dev_type.value)
            .map(|s| s != "cpu")
            .unwrap_or(false)
        {
            return false;
        }
    } else if cpu.name != "cpu" && !cpu.name.starts_with("cpu@") {
        return false;
    }

    if let Some(status) = cpu.property("status") {
        if bytes_to_cstr(status.value)
            .map(|s| s == "disabled")
            .unwrap_or(false)
        {
            return false;
        }
    }

    true
}

fn isa_extension_list_has_extension(list: &[u8], extension: &[u8]) -> bool {
    let mut i = 0;
    while i < list.len() {
        let end = list[i..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| i + p)
            .unwrap_or(list.len());
        let tok = &list[i..end];
        if tok.is_empty() {
            break;
        }
        if riscv_token_matches_extension(tok, extension) {
            return true;
        }
        if end == list.len() {
            break;
        }
        i = end + 1;
    }

    false
}

fn isa_string_has_extension(isa: &[u8], extension: &[u8]) -> bool {
    let len = isa.iter().position(|&b| b == 0).unwrap_or(isa.len());
    let isa = &isa[..len];
    if isa.len() < 4 {
        return false;
    }

    let b0 = isa[0] | 0x20;
    let b1 = isa[1] | 0x20;
    if b0 != b'r' || b1 != b'v' {
        return false;
    }

    let start = 4;
    let mut token_start = start;
    while token_start < isa.len() {
        let token_end = isa[token_start..]
            .iter()
            .position(|&b| b == b'_')
            .map(|p| token_start + p)
            .unwrap_or(isa.len());
        let token = &isa[token_start..token_end];

        if token_start > start && riscv_token_matches_extension(token, extension) {
            return true;
        }

        if token_end == isa.len() {
            break;
        }
        token_start = token_end + 1;
    }

    false
}

fn riscv_token_matches_extension(token: &[u8], extension: &[u8]) -> bool {
    if token.len() < extension.len() {
        return false;
    }

    for (token_byte, extension_byte) in token.iter().zip(extension.iter()) {
        if (*token_byte | 0x20) != (*extension_byte | 0x20) {
            return false;
        }
    }

    token.len() == extension.len() || token[extension.len()].is_ascii_digit()
}

fn bytes_to_cstr(bytes: &[u8]) -> Option<&str> {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).ok()
}
