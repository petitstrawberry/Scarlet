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

#[cfg(feature = "v2-t0")]
const SELECTED_VARIANT_NAME: &str = "v2-t0";
#[cfg(all(not(feature = "v2-t0"), feature = "v3-t1"))]
const SELECTED_VARIANT_NAME: &str = "v3-t1";
#[cfg(all(not(feature = "v2-t0"), not(feature = "v3-t1"), feature = "v4-t0"))]
const SELECTED_VARIANT_NAME: &str = "v4-t0";
#[cfg(all(
    not(feature = "v2-t0"),
    not(feature = "v3-t1"),
    not(feature = "v4-t0"),
    feature = "v5-t0"
))]
const SELECTED_VARIANT_NAME: &str = "v5-t0";
#[cfg(all(
    not(feature = "v2-t0"),
    not(feature = "v3-t1"),
    not(feature = "v4-t0"),
    not(feature = "v5-t0"),
    feature = "v5-t1"
))]
const SELECTED_VARIANT_NAME: &str = "v5-t1";
#[cfg(all(
    not(feature = "v2-t0"),
    not(feature = "v3-t1"),
    not(feature = "v4-t0"),
    not(feature = "v5-t0"),
    not(feature = "v5-t1")
))]
const SELECTED_VARIANT_NAME: &str = "v3-t0";

const SELECTED_TUNABLES: &[Tunable] = &[];

/// Return the compile-time-selected AVD firmware variant.
///
/// # Returns
///
/// Selected variant descriptor.
pub const fn selected_variant() -> FirmwareVariant {
    FirmwareVariant {
        name: SELECTED_VARIANT_NAME,
        tunables: SELECTED_TUNABLES,
    }
}

/// Apply tunables for the selected firmware variant.
pub fn apply_selected_tunables() {
    let variant = selected_variant();
    let _ = variant.name;
    for tunable in variant.tunables {
        let ptr = tunable.offset as usize as *mut u32;
        // SAFETY: Tunable offsets are version-specific AVD-local MMIO addresses.
        unsafe {
            core::ptr::write_volatile(ptr, tunable.value);
        }
    }
}
