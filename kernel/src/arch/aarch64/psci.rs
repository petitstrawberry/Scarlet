//! PSCI conduit selection shared by CPU startup and system power control.

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

pub(crate) const SYSTEM_OFF: u64 = 0x8400_0008;
pub(crate) const SYSTEM_RESET: u64 = 0x8400_0009;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Conduit {
    Smc,
    Hvc,
}

impl Conduit {
    pub(crate) fn from_fdt(fdt: &fdt::Fdt<'_>) -> Option<Self> {
        let node = fdt.all_nodes().find(|node| {
            node.property("status")
                .and_then(|property| property.as_str())
                .is_none_or(|status| matches!(status, "okay" | "ok"))
                && node.compatible().is_some_and(|compatible| {
                    compatible
                        .all()
                        .any(|value| matches!(value, "arm,psci-0.2" | "arm,psci-1.0"))
                })
        })?;
        match node.property("method")?.as_str()? {
            "smc" => Some(Self::Smc),
            "hvc" => Some(Self::Hvc),
            _ => None,
        }
    }

    #[cfg(target_arch = "aarch64")]
    pub(crate) fn invoke(self, function: u64, arg0: u64, arg1: u64, arg2: u64) -> i64 {
        let mut result = function;
        // SAFETY: The enabled PSCI firmware node selected the conduit. Use
        // the SMCCC register ABI, including caller-clobbered registers and
        // a memory barrier in the compiler model (firmware starts a CPU).
        unsafe {
            match self {
                Self::Smc => asm!(
                    "smc #0",
                    inout("x0") result,
                    inout("x1") arg0 => _,
                    inout("x2") arg1 => _,
                    inout("x3") arg2 => _,
                    in("x4") 0u64, in("x5") 0u64,
                    in("x6") 0u64, in("x7") 0u64,
                    clobber_abi("C"),
                    options(nostack),
                ),
                Self::Hvc => asm!(
                    "hvc #0",
                    inout("x0") result,
                    inout("x1") arg0 => _,
                    inout("x2") arg1 => _,
                    inout("x3") arg2 => _,
                    in("x4") 0u64, in("x5") 0u64,
                    in("x6") 0u64, in("x7") 0u64,
                    clobber_abi("C"),
                    options(nostack),
                ),
            }
        }
        // Error codes are signed 32-bit values, including for SMC64 calls.
        result as i32 as i64
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn dtb(method: Option<&str>, status: Option<&str>, compatible: &str) -> Vec<u8> {
        fn word(out: &mut Vec<u8>, value: u32) {
            out.extend_from_slice(&value.to_be_bytes());
        }
        fn name(out: &mut Vec<u8>, value: &str) {
            out.extend_from_slice(value.as_bytes());
            out.push(0);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        }
        fn property(out: &mut Vec<u8>, offset: u32, value: &str) {
            word(out, 3);
            word(out, value.len() as u32 + 1);
            word(out, offset);
            name(out, value);
        }
        let mut nodes = Vec::new();
        word(&mut nodes, 1);
        name(&mut nodes, "");
        word(&mut nodes, 1);
        name(&mut nodes, "psci");
        property(&mut nodes, 0, compatible);
        if let Some(method) = method {
            property(&mut nodes, 11, method);
        }
        if let Some(status) = status {
            property(&mut nodes, 18, status);
        }
        for token in [2, 2, 9] {
            word(&mut nodes, token);
        }
        let strings = b"compatible\0method\0status\0";
        let mut data = Vec::new();
        for value in [
            0xd00dfeed,
            (56 + nodes.len() + strings.len()) as u32,
            56,
            (56 + nodes.len()) as u32,
            40,
            17,
            16,
            0,
            strings.len() as u32,
            nodes.len() as u32,
        ] {
            word(&mut data, value);
        }
        data.extend_from_slice(&[0; 16]);
        data.extend(nodes);
        data.extend(strings);
        data
    }
    #[test_case]
    fn firmware_method_selects_smc_and_hvc_for_supported_versions() {
        for compatible in ["arm,psci-0.2", "arm,psci-1.0"] {
            for (method, expected) in [("smc", Conduit::Smc), ("hvc", Conduit::Hvc)] {
                for status in [None, Some("okay"), Some("ok")] {
                    let blob = dtb(Some(method), status, compatible);
                    assert_eq!(
                        Conduit::from_fdt(&fdt::Fdt::new(&blob).unwrap()),
                        Some(expected)
                    );
                }
            }
        }
    }
    #[test_case]
    fn missing_unknown_disabled_or_legacy_configuration_never_guesses_hvc() {
        for (method, status, compatible) in vec![
            (None, None, "arm,psci-1.0"),
            (Some("invalid"), None, "arm,psci-1.0"),
            (Some("hvc"), Some("disabled"), "arm,psci-1.0"),
            (Some("smc"), Some("fail"), "arm,psci-1.0"),
            (Some("smc"), None, "arm,psci"),
        ] {
            let blob = dtb(method, status, compatible);
            assert_eq!(Conduit::from_fdt(&fdt::Fdt::new(&blob).unwrap()), None);
        }
    }
}
