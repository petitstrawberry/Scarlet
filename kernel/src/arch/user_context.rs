//! Runtime gating for user-mode FPU / Vector context handling.
//!
//! This module complements Cargo feature gates (`user-fpu`, `user-vector`) with
//! a DTB(FDT)-driven runtime switch. When the relevant Cargo feature is enabled,
//! the kernel will decide at boot whether user-mode access should actually be
//! allowed based on information found in the device tree.
//!
//! ## DTB overrides
//!
//! If present under `/chosen`, the following properties can disable detected hardware support:
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

    // Policy can disable supported context state, never enable absent hardware.
    if let Some(fdt) = FdtManager::get_manager().get_fdt() {
        if let Some(chosen) = fdt.find_node("/chosen") {
            if let Some(v) = read_boolish_property(&chosen, "scarlet,user-fpu") {
                enable_fpu &= v;
            }
            if let Some(v) = read_boolish_property(&chosen, "scarlet,user-vector") {
                enable_vector &= v;
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
    crate::println!(
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
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        use crate::arch::riscv::fdt::all_cpus_have_isa_extension_from_fdt as all_have;
        // The context format saves 64-bit F registers with fld/fsd, requiring
        // D. Full vector context uses V; partial Zve/Zvl support is insufficient.
        (
            all_have("d").unwrap_or(false),
            all_have("v").unwrap_or(false),
        )
    }
    #[cfg(target_arch = "aarch64")]
    {
        (true, false)
    }
}
