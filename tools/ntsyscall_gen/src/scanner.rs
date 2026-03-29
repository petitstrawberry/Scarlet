use std::collections::BTreeMap;
use std::path::Path;

use crate::pe::{ExportedFunction, PeFile};

const RET_INSN: u32 = 0xD65F_03C0;

#[derive(Debug, Clone)]
pub struct SyscallEntry {
    pub number: u16,
    pub name: String,
    pub rva: u32,
}

pub fn scan_nt_syscalls(pe: &PeFile) -> Result<Vec<SyscallEntry>, String> {
    let exports = pe.exported_functions();
    if exports.is_empty() {
        return Ok(Vec::new());
    }

    let rva_limits = function_rva_limits(exports);
    let mut by_name: BTreeMap<String, SyscallEntry> = BTreeMap::new();

    for export in exports {
        let Some(start) = pe.rva_to_offset(export.rva) else {
            continue;
        };
        let Some(end_rva) = rva_limits.get(&export.rva).copied() else {
            continue;
        };

        let max_len = end_rva.saturating_sub(export.rva) as usize;
        if max_len < 4 {
            continue;
        }

        let end = start.saturating_add(max_len).min(pe.bytes().len());
        if end <= start + 4 {
            continue;
        }

        let code = &pe.bytes()[start..end];
        if let Some(number) = extract_syscall_number(code) {
            let canonical_name = canonical_nt_name(&export.name);
            by_name
                .entry(canonical_name.clone())
                .or_insert(SyscallEntry {
                    number,
                    name: canonical_name,
                    rva: export.rva,
                });
        }
    }

    let mut out: Vec<SyscallEntry> = by_name.into_values().collect();
    out.sort_by(|a, b| a.number.cmp(&b.number).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

fn function_rva_limits(exports: &[ExportedFunction]) -> BTreeMap<u32, u32> {
    let mut by_rva: Vec<u32> = exports.iter().map(|e| e.rva).collect();
    by_rva.sort_unstable();
    by_rva.dedup();

    let mut limits = BTreeMap::new();
    for (idx, rva) in by_rva.iter().enumerate() {
        let next = by_rva
            .get(idx + 1)
            .copied()
            .unwrap_or_else(|| rva.saturating_add(64));
        limits.insert(*rva, next);
    }
    limits
}

fn extract_syscall_number(code: &[u8]) -> Option<u16> {
    let max_scan = code.len().min(128);
    let mut i = 0usize;
    let mut mov_x8_imm: Option<u16> = None;

    while i + 4 <= max_scan {
        let insn = u32::from_le_bytes([code[i], code[i + 1], code[i + 2], code[i + 3]]);

        if is_movz_x8(insn) {
            mov_x8_imm = Some(((insn >> 5) & 0xFFFF) as u16);
        }

        if is_svc(insn) {
            let imm16 = ((insn >> 5) & 0xFFFF) as u16;

            let maybe_next = if i + 8 <= code.len() {
                Some(u32::from_le_bytes([
                    code[i + 4],
                    code[i + 5],
                    code[i + 6],
                    code[i + 7],
                ]))
            } else {
                None
            };

            if let Some(next) = maybe_next {
                if next == RET_INSN {
                    if imm16 == 0 {
                        if let Some(v) = mov_x8_imm {
                            return Some(v);
                        }
                    }
                    return Some(imm16);
                }
            }

            if imm16 != 0 {
                return Some(imm16);
            }
            if let Some(v) = mov_x8_imm {
                return Some(v);
            }
        }

        i += 4;
    }

    None
}

fn is_svc(insn: u32) -> bool {
    (insn & 0xFFE0_001F) == 0xD400_0001
}

fn is_movz_x8(insn: u32) -> bool {
    (insn & 0xFFE0_001F) == 0xD280_0008
}

/// Parse a hfiref0x/SyscallTables text file (tab-separated: `Name\tNumber`).
///
/// Each line is expected to have exactly two fields separated by a tab.
/// Blank lines and lines starting with `#` are skipped.
pub fn parse_text_table(path: &Path) -> Result<Vec<SyscallEntry>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;

    let mut entries = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() != 2 {
            return Err(format!(
                "invalid line {lineno}: expected 'Name\\tNumber', got: {line}"
            ));
        }
        let name = parts[0].trim().to_string();
        let number: u16 = parts[1].trim().parse().map_err(|e| {
            let val = parts[1].trim();
            format!("invalid syscall number on line {lineno}: '{val}': {e}")
        })?;
        entries.push(SyscallEntry {
            number,
            name,
            rva: 0,
        });
    }

    entries.sort_by(|a, b| a.number.cmp(&b.number).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

fn canonical_nt_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("Zw") {
        format!("Nt{rest}")
    } else {
        name.to_string()
    }
}
