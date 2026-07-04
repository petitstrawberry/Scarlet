//! AVD version/tier-specific firmware tunables.

/// One firmware tunable register write.
#[derive(Clone, Copy)]
pub struct Tunable {
    /// Register offset inside the AVD-local tunable window.
    pub offset: u32,
    /// Value to write.
    pub value: u32,
}

/// Selected AVD firmware variant.
#[derive(Clone, Copy)]
pub struct FirmwareVariant {
    /// Human-readable variant name.
    pub name: &'static str,
    /// Tunable writes required before enabling decode IRQs.
    pub tunables: &'static [Tunable],
}

const V2_T0_TUNABLES: &[Tunable] = &[];
const V3_T0_TUNABLES: &[Tunable] = &[];
const V3_T1_TUNABLES: &[Tunable] = &[];
const V4_T0_TUNABLES: &[Tunable] = &[];
const V5_T0_TUNABLES: &[Tunable] = &[];
const V5_T1_TUNABLES: &[Tunable] = &[];

/// Return the compile-time-selected AVD firmware variant.
///
/// # Returns
///
/// Selected variant descriptor.
pub const fn selected_variant() -> FirmwareVariant {
    #[cfg(feature = "v2-t0")]
    {
        return FirmwareVariant {
            name: "v2-t0",
            tunables: V2_T0_TUNABLES,
        };
    }
    #[cfg(feature = "v3-t1")]
    {
        return FirmwareVariant {
            name: "v3-t1",
            tunables: V3_T1_TUNABLES,
        };
    }
    #[cfg(feature = "v4-t0")]
    {
        return FirmwareVariant {
            name: "v4-t0",
            tunables: V4_T0_TUNABLES,
        };
    }
    #[cfg(feature = "v5-t0")]
    {
        return FirmwareVariant {
            name: "v5-t0",
            tunables: V5_T0_TUNABLES,
        };
    }
    #[cfg(feature = "v5-t1")]
    {
        return FirmwareVariant {
            name: "v5-t1",
            tunables: V5_T1_TUNABLES,
        };
    }
    FirmwareVariant {
        name: "v3-t0",
        tunables: V3_T0_TUNABLES,
    }
}

/// Apply tunables for the selected firmware variant.
pub fn apply_selected_tunables() {
    let variant = selected_variant();
    for tunable in variant.tunables {
        let ptr = tunable.offset as usize as *mut u32;
        // SAFETY: Tunable offsets are version-specific AVD-local MMIO addresses.
        unsafe {
            core::ptr::write_volatile(ptr, tunable.value);
        }
    }
}
