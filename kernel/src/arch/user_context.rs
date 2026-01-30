//! Runtime gating for user-mode FPU / Vector context handling.
//!
//! This module complements Cargo feature gates (`user-fpu`, `user-vector`) with
//! a DTB(FDT)-driven runtime switch. When the relevant Cargo feature is enabled,
//! the kernel will decide at boot whether user-mode access should actually be
//! allowed based on information found in the device tree.
//!
//! ## DTB overrides
//!
//! If present under `/chosen`, the following properties override auto-detection:
//!
//! - `scarlet,user-fpu` (boolean or u32/u64)
//! - `scarlet,user-vector` (boolean or u32/u64)
//!
//! When absent, the kernel attempts architecture-specific detection when
//! available (currently implemented for RISC-V via `riscv,isa`).

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::device::fdt::FdtManager;

const FLAG_USER_FPU: u8 = 1 << 0;
const FLAG_USER_VECTOR: u8 = 1 << 1;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static FLAGS: AtomicU8 = AtomicU8::new(0);

/// Initialize runtime user-context switches from the current DTB (FDT).
///
/// This function is idempotent; only the first call performs initialization.
pub fn init_from_fdt() {
    if INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let (mut enable_fpu, mut enable_vector) = arch_defaults_from_fdt();

    // `/chosen` overrides win.
    if let Some(fdt) = FdtManager::get_manager().get_fdt() {
        if let Some(chosen) = fdt.find_node("/chosen") {
            if let Some(v) = read_boolish_property(&chosen, "scarlet,user-fpu") {
                enable_fpu = v;
            }
            if let Some(v) = read_boolish_property(&chosen, "scarlet,user-vector") {
                enable_vector = v;
            }
        }
    }

    let mut flags = 0u8;
    if cfg!(feature = "user-fpu") && enable_fpu {
        flags |= FLAG_USER_FPU;
    }
    if cfg!(feature = "user-vector") && enable_vector {
        flags |= FLAG_USER_VECTOR;
    }
    FLAGS.store(flags, Ordering::Release);

    // Keep this log short; it is useful when bringing up new boards.
    crate::early_println!(
        "[userctx] enabled: user-fpu={} user-vector={}",
        user_fpu_enabled(),
        user_vector_enabled()
    );
}

/// Returns whether user-mode FPU context handling is enabled (feature + DTB).
#[inline]
pub fn user_fpu_enabled() -> bool {
    cfg!(feature = "user-fpu") && (FLAGS.load(Ordering::Acquire) & FLAG_USER_FPU != 0)
}

/// Returns whether user-mode Vector context handling is enabled (feature + DTB).
#[inline]
pub fn user_vector_enabled() -> bool {
    cfg!(feature = "user-vector") && (FLAGS.load(Ordering::Acquire) & FLAG_USER_VECTOR != 0)
}

fn read_boolish_property(node: &fdt::node::FdtNode, name: &str) -> Option<bool> {
    let prop = node.property(name)?;

    // Boolean property: present with empty value.
    if prop.value.is_empty() {
        return Some(true);
    }

    match prop.value.len() {
        4 => Some(u32::from_be_bytes(prop.value[0..4].try_into().ok()?) != 0),
        8 => Some(u64::from_be_bytes(prop.value[0..8].try_into().ok()?) != 0),
        _ => {
            // Try string forms like "0" / "1".
            let s = bytes_to_cstr(prop.value)?;
            match s.trim() {
                "0" | "false" | "no" | "off" => Some(false),
                "1" | "true" | "yes" | "on" => Some(true),
                _ => None,
            }
        }
    }
}

fn bytes_to_cstr(bytes: &[u8]) -> Option<&str> {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).ok()
}

fn arch_defaults_from_fdt() -> (bool, bool) {
    // Default policy:
    // - If we can detect from DTB, use it.
    // - Otherwise be conservative for Vector (disabled) and permissive for FPU.

    #[cfg(target_arch = "riscv64")]
    {
        if let Some(fdt) = FdtManager::get_manager().get_fdt() {
            if let Some((has_fpu, has_vec)) = riscv_extensions_from_fdt(fdt) {
                return (has_fpu, has_vec);
            }
        }
        return (true, false);
    }

    #[cfg(target_arch = "aarch64")]
    {
        // Most AArch64 platforms provide FP/SIMD; DTB rarely encodes it.
        // Keep FP enabled by default (when compiled), and keep "vector" disabled.
        return (true, false);
    }

    #[cfg(not(any(target_arch = "riscv64", target_arch = "aarch64")))]
    {
        (true, false)
    }
}

#[cfg(target_arch = "riscv64")]
fn riscv_extensions_from_fdt(fdt: &fdt::Fdt) -> Option<(bool, bool)> {
    let cpus = fdt.find_node("/cpus")?;

    let mut saw_cpu = false;
    let mut all_have_fpu = true;
    let mut all_have_vec = true;

    for cpu in cpus.children() {
        // Filter to CPU nodes when possible.
        if let Some(dev_type) = cpu.property("device_type") {
            if bytes_to_cstr(dev_type.value)
                .map(|s| s != "cpu")
                .unwrap_or(false)
            {
                continue;
            }
        }

        // Prefer the modern string list if present; fall back to the legacy ISA string.
        let (has_fpu, has_vec) = if let Some(p) = cpu.property("riscv,isa-extensions") {
            riscv_extensions_from_isa_extensions(p.value)?
        } else if let Some(p) = cpu.property("riscv,isa") {
            riscv_extensions_from_isa_string(p.value)?
        } else {
            continue;
        };

        saw_cpu = true;
        all_have_fpu &= has_fpu;
        all_have_vec &= has_vec;
    }

    if saw_cpu {
        Some((all_have_fpu, all_have_vec))
    } else {
        None
    }
}

#[cfg(target_arch = "riscv64")]
fn riscv_extensions_from_isa_string(isa: &[u8]) -> Option<(bool, bool)> {
    let len = isa.iter().position(|&b| b == 0).unwrap_or(isa.len());
    let isa = &isa[..len];
    if isa.len() < 4 {
        return None;
    }

    // Expect "rv32" or "rv64".
    let b0 = isa[0] | 0x20;
    let b1 = isa[1] | 0x20;
    if b0 != b'r' || b1 != b'v' {
        return None;
    }

    // Single-letter extensions are the contiguous run right after rv32/rv64
    // and before the first '_' token separator.
    let start = 4;
    let mut end = isa.len();
    for (i, &b) in isa[start..].iter().enumerate() {
        if b == b'_' {
            end = start + i;
            break;
        }
    }

    let letters = &isa[start..end];
    let has_fpu = letters.iter().any(|&b| {
        let c = b | 0x20;
        c == b'f' || c == b'd'
    });

    // Vector is either the single-letter 'v' (in the letters block) or an explicit
    // token like "v" / "zve*" / "zvl*" in the '_' separated extension list.
    let mut has_vec = letters.iter().any(|&b| (b | 0x20) == b'v');

    if !has_vec && end < isa.len() {
        has_vec = riscv_isa_suffix_has_vector_tokens(&isa[end + 1..]);
    }

    Some((has_fpu, has_vec))
}

#[cfg(target_arch = "riscv64")]
fn riscv_isa_suffix_has_vector_tokens(mut s: &[u8]) -> bool {
    // Suffix is '_' separated. Tokens like "svvptc" must NOT be treated as V.
    while !s.is_empty() {
        let next = s.iter().position(|&b| b == b'_').unwrap_or(s.len());
        let tok = &s[..next];
        if riscv_token_is_vector(tok) {
            return true;
        }
        s = if next < s.len() { &s[next + 1..] } else { &[] };
    }
    false
}

#[cfg(target_arch = "riscv64")]
fn riscv_extensions_from_isa_extensions(list: &[u8]) -> Option<(bool, bool)> {
    // DTB string list: multiple NUL-terminated strings packed together.
    let mut has_fpu = false;
    let mut has_vec = false;

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
        if riscv_token_is_fpu(tok) {
            has_fpu = true;
        }
        if riscv_token_is_vector(tok) {
            has_vec = true;
        }
        if end == list.len() {
            break;
        }
        i = end + 1;
    }

    Some((has_fpu, has_vec))
}

#[cfg(target_arch = "riscv64")]
fn riscv_token_is_fpu(tok: &[u8]) -> bool {
    tok.len() == 1 && {
        let c = tok[0] | 0x20;
        c == b'f' || c == b'd'
    }
}

#[cfg(target_arch = "riscv64")]
fn riscv_token_is_vector(tok: &[u8]) -> bool {
    if tok.is_empty() {
        return false;
    }
    // Normalize to lowercase without allocation by checking bytes.
    if tok.len() == 1 && ((tok[0] | 0x20) == b'v') {
        return true;
    }
    if tok.len() >= 3 {
        let t0 = tok[0] | 0x20;
        let t1 = tok[1] | 0x20;
        let t2 = tok[2] | 0x20;
        // Any Zve* or Zvl* implies some form of vector support.
        if t0 == b'z' && t1 == b'v' && (t2 == b'e' || t2 == b'l') {
            return true;
        }
    }
    false
}
