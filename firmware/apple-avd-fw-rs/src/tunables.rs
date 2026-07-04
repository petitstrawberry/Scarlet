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

const COMMON_H264_TUNABLES: &[Tunable] = &[
    Tunable {
        offset: 0x0011_4018,
        value: 0x78,
    },
    Tunable {
        offset: 0x0011_401c,
        value: 0x78,
    },
    Tunable {
        offset: 0x0011_4020,
        value: 0x78,
    },
    Tunable {
        offset: 0x0011_4024,
        value: 0x78,
    },
    Tunable {
        offset: 0x0011_4028,
        value: 0x20,
    },
    Tunable {
        offset: 0x0011_4034,
        value: selected_tier_value(),
    },
    Tunable {
        offset: 0x0011_403c,
        value: 0,
    },
    Tunable {
        offset: 0x0011_405c,
        value: 0x0050_0000,
    },
    Tunable {
        offset: 0x0011_4060,
        value: 0x0084_2108,
    },
    Tunable {
        offset: 0x0011_4064,
        value: 0x3,
    },
    Tunable {
        offset: 0x0010_8ee90,
        value: 0x0402_0002,
    },
    Tunable {
        offset: 0x0010_8ee94,
        value: 0x0002_0002,
    },
    Tunable {
        offset: 0x0010_8ee98,
        value: 0x0402_0002,
    },
    Tunable {
        offset: 0x0010_8ee9c,
        value: 0x0402_0002,
    },
    Tunable {
        offset: 0x0010_8eea0,
        value: 0x0402_0002,
    },
    Tunable {
        offset: 0x0010_8eea4,
        value: 0x0007_0007,
    },
    Tunable {
        offset: 0x0010_8eea8,
        value: 0x0007_0007,
    },
    Tunable {
        offset: 0x0010_8eeac,
        value: 0x0007_0007,
    },
    Tunable {
        offset: 0x0010_8eeb0,
        value: 0x0007_0007,
    },
    Tunable {
        offset: 0x0010_8eeb4,
        value: 0x0007_0007,
    },
];

const SELECTED_TUNABLES: &[Tunable] = COMMON_H264_TUNABLES;

const fn selected_tier_value() -> u32 {
    #[cfg(any(feature = "v3-t1", feature = "v5-t1"))]
    {
        1
    }
    #[cfg(not(any(feature = "v3-t1", feature = "v5-t1")))]
    {
        0
    }
}

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
