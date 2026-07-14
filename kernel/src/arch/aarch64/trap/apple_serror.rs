//! Apple-specific asynchronous SError diagnostics.

use core::arch::asm;

const APPLE_MIDR_IMPLEMENTER: u64 = 0x61;
const MIDR_IMPLEMENTER_SHIFT: u32 = 24;
const MPIDR_PERFORMANCE_CLUSTER_BIT: u64 = 1 << 16;
const L2C_ERR_STS_RECURSIVE_FAULT: u64 = 1 << 1;
const L2C_ERR_STS_ACCESS_FAULT: u64 = 1 << 7;
const L2C_ERR_ADR_ADDRESS_MASK: u64 = (1 << 42) - 1;
const L2C_ERR_ADR_COMMAND_SHIFT: u32 = 53;
const L2C_ERR_ADR_COMMAND_MASK: u64 = 0x7f;

macro_rules! read_apple_sysreg {
    ($register:literal) => {{
        let value: u64;
        // SAFETY: The caller verifies Apple's MIDR implementer before expanding
        // this macro, and the selected register encodings are implemented by
        // Apple Silicon CPUs.
        unsafe {
            asm!(
                concat!("mrs {value}, ", $register),
                value = out(reg) value,
                options(nomem, nostack, preserves_flags),
            );
        }
        value
    }};
}

macro_rules! write_apple_sysreg {
    ($register:literal, $value:expr) => {{
        // SAFETY: The caller verifies Apple's MIDR implementer before expanding
        // this macro, and writes the value read from the W1C status register.
        unsafe {
            asm!(
                concat!("msr ", $register, ", {value}"),
                value = in(reg) $value,
                options(nomem, nostack, preserves_flags),
            );
        }
    }};
}

fn read_midr_el1() -> u64 {
    let value: u64;
    // SAFETY: MIDR_EL1 is an architected, read-only AArch64 system register.
    unsafe {
        asm!(
            "mrs {value}, midr_el1",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

fn read_mpidr_el1() -> u64 {
    let value: u64;
    // SAFETY: MPIDR_EL1 is an architected, read-only AArch64 system register.
    unsafe {
        asm!(
            "mrs {value}, mpidr_el1",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

fn is_apple_cpu(midr: u64) -> bool {
    ((midr >> MIDR_IMPLEMENTER_SHIFT) & 0xff) == APPLE_MIDR_IMPLEMENTER
}

fn is_efficiency_core(mpidr: u64) -> bool {
    mpidr & MPIDR_PERFORMANCE_CLUSTER_BIT == 0
}

fn l2c_error_address(raw: u64) -> u64 {
    raw & L2C_ERR_ADR_ADDRESS_MASK
}

fn l2c_error_command(raw: u64) -> u64 {
    (raw >> L2C_ERR_ADR_COMMAND_SHIFT) & L2C_ERR_ADR_COMMAND_MASK
}

pub(super) fn print_error_status() {
    let midr = read_midr_el1();
    if !is_apple_cpu(midr) {
        crate::println!(
            "[serror] Apple error-status registers skipped for MIDR={:#x}",
            midr
        );
        return;
    }

    let mpidr = read_mpidr_el1();
    let l2c_status = read_apple_sysreg!("S3_3_C15_C8_0");
    let l2c_address = read_apple_sysreg!("S3_3_C15_C9_0");
    let l2c_info = read_apple_sysreg!("S3_3_C15_C10_0");
    let (core_kind, lsu_status, fed_status) = if is_efficiency_core(mpidr) {
        (
            "E",
            read_apple_sysreg!("S3_3_C15_C2_0"),
            read_apple_sysreg!("S3_4_C15_C0_2"),
        )
    } else {
        (
            "P",
            read_apple_sysreg!("S3_3_C15_C0_0"),
            read_apple_sysreg!("S3_4_C15_C0_0"),
        )
    };

    crate::println!(
        "[apple-serror] MIDR={:#x} MPIDR={:#x} core={}",
        midr,
        mpidr,
        core_kind
    );
    crate::println!(
        "[apple-serror] L2C_ERR_STS={:#018x} L2C_ERR_ADR={:#018x} L2C_ERR_INF={:#018x}",
        l2c_status,
        l2c_address,
        l2c_info
    );
    crate::println!(
        "[apple-serror] {}_LSU_ERR_STS={:#018x} {}_FED_ERR_STS={:#018x} access_fault={} recursive={} cmd={:#x} paddr={:#x}",
        core_kind,
        lsu_status,
        core_kind,
        fed_status,
        l2c_status & L2C_ERR_STS_ACCESS_FAULT != 0,
        l2c_status & L2C_ERR_STS_RECURSIVE_FAULT != 0,
        l2c_error_command(l2c_address),
        l2c_error_address(l2c_address),
    );

    write_apple_sysreg!("S3_3_C15_C8_0", l2c_status);
    // SAFETY: Complete the W1C status write before returning from the SError handler.
    unsafe { asm!("dsb sy", "isb", options(nostack, preserves_flags)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn detects_apple_midr_implementer() {
        assert!(is_apple_cpu(
            APPLE_MIDR_IMPLEMENTER << MIDR_IMPLEMENTER_SHIFT
        ));
        assert!(!is_apple_cpu(0x41 << MIDR_IMPLEMENTER_SHIFT));
    }

    #[test_case]
    fn classifies_apple_cluster_from_mpidr() {
        assert!(is_efficiency_core(0));
        assert!(!is_efficiency_core(MPIDR_PERFORMANCE_CLUSTER_BIT));
    }

    #[test_case]
    fn decodes_l2c_error_command_and_address() {
        let raw = 0x0280_040e_0010_2240;

        assert_eq!(l2c_error_command(raw), 0x14);
        assert_eq!(l2c_error_address(raw), 0x0e_0010_2240);
    }
}
