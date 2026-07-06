//! AVD version/tier-specific firmware tunables.

/// One firmware tunable register update.
#[derive(Clone, Copy)]
pub struct Tunable {
    /// Register offset inside the CM3-visible decode control block.
    pub offset: u32,
    /// Bit mask to replace.
    pub mask: u32,
    /// Value to apply under `mask`.
    pub value: u32,
}

/// Selected AVD firmware variant.
#[derive(Clone, Copy)]
pub struct FirmwareVariant {
    /// Human-readable variant name.
    pub name: &'static str,
    /// CM3-visible decode control block base.
    pub decode_ctrl_base: usize,
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

#[cfg(any(feature = "v2-t0", feature = "v3-t0", feature = "v3-t1"))]
const DECODE_CTRL_BASE: usize = 0x4010_0000;
#[cfg(any(feature = "v4-t0", feature = "v5-t0", feature = "v5-t1"))]
const DECODE_CTRL_BASE: usize = 0x4110_0000;

const V2_T0_TUNABLES: &[Tunable] = &[
    Tunable {
        offset: 0x0000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x1000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x2000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x3000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x4000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x5000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x6000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x7000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x8000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0x9000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xa000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xb000,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc010,
        mask: 0x00000001,
        value: 0x00000001,
    },
    Tunable {
        offset: 0xc018,
        mask: 0x00000001,
        value: 0x00000001,
    },
    Tunable {
        offset: 0xc040,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc044,
        mask: 0xffffffff,
        value: 0x00000040,
    },
    Tunable {
        offset: 0xc080,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc084,
        mask: 0xffffffff,
        value: 0x00400040,
    },
    Tunable {
        offset: 0xc0c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc100,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc140,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc180,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc1c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc1c4,
        mask: 0xffffffff,
        value: 0x006c0048,
    },
    Tunable {
        offset: 0xc200,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc204,
        mask: 0xffffffff,
        value: 0x00b40048,
    },
    Tunable {
        offset: 0xc240,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc244,
        mask: 0xffffffff,
        value: 0x00800034,
    },
    Tunable {
        offset: 0xc280,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc284,
        mask: 0xffffffff,
        value: 0x00000018,
    },
    Tunable {
        offset: 0xc2c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc2c4,
        mask: 0xffffffff,
        value: 0x00b40020,
    },
    Tunable {
        offset: 0xc300,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc340,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc380,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc384,
        mask: 0xffffffff,
        value: 0x00fc0038,
    },
    Tunable {
        offset: 0xc3c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc3c4,
        mask: 0xffffffff,
        value: 0x00d40030,
    },
    Tunable {
        offset: 0xc400,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc404,
        mask: 0xffffffff,
        value: 0x00180014,
    },
    Tunable {
        offset: 0xc440,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc444,
        mask: 0xffffffff,
        value: 0x0104001c,
    },
    Tunable {
        offset: 0xc480,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc484,
        mask: 0xffffffff,
        value: 0x002c0014,
    },
    Tunable {
        offset: 0xc4c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc4c4,
        mask: 0xffffffff,
        value: 0x01200014,
    },
    Tunable {
        offset: 0xc500,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc504,
        mask: 0xffffffff,
        value: 0x00400018,
    },
    Tunable {
        offset: 0xc540,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc544,
        mask: 0xffffffff,
        value: 0x01340024,
    },
    Tunable {
        offset: 0xc580,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc584,
        mask: 0xffffffff,
        value: 0x00580014,
    },
    Tunable {
        offset: 0xc5c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc5c4,
        mask: 0xffffffff,
        value: 0x01580014,
    },
    Tunable {
        offset: 0xc600,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc604,
        mask: 0xffffffff,
        value: 0x01340030,
    },
    Tunable {
        offset: 0xc640,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc644,
        mask: 0xffffffff,
        value: 0x016c00b0,
    },
    Tunable {
        offset: 0xc680,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc684,
        mask: 0xffffffff,
        value: 0x021c00b0,
    },
    Tunable {
        offset: 0xc6c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc700,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc740,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc744,
        mask: 0xffffffff,
        value: 0x01800018,
    },
    Tunable {
        offset: 0xc780,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc784,
        mask: 0xffffffff,
        value: 0x02f40020,
    },
    Tunable {
        offset: 0xc7c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc7c4,
        mask: 0xffffffff,
        value: 0x01980018,
    },
    Tunable {
        offset: 0xc800,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc804,
        mask: 0xffffffff,
        value: 0x0314001c,
    },
    Tunable {
        offset: 0xc840,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc844,
        mask: 0xffffffff,
        value: 0x0164001c,
    },
    Tunable {
        offset: 0xc880,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc884,
        mask: 0xffffffff,
        value: 0x02cc0028,
    },
    Tunable {
        offset: 0xc8c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc8c4,
        mask: 0xffffffff,
        value: 0x01b00024,
    },
    Tunable {
        offset: 0xc900,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc904,
        mask: 0xffffffff,
        value: 0x03300040,
    },
    Tunable {
        offset: 0xc940,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc944,
        mask: 0xffffffff,
        value: 0x01d4001c,
    },
    Tunable {
        offset: 0xc980,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc984,
        mask: 0xffffffff,
        value: 0x0370002c,
    },
    Tunable {
        offset: 0xc9c0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xc9c4,
        mask: 0xffffffff,
        value: 0x01f00030,
    },
    Tunable {
        offset: 0xca00,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xca04,
        mask: 0xffffffff,
        value: 0x039c003c,
    },
    Tunable {
        offset: 0xca40,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xca44,
        mask: 0xffffffff,
        value: 0x02200014,
    },
    Tunable {
        offset: 0xca80,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xca84,
        mask: 0xffffffff,
        value: 0x03d80014,
    },
    Tunable {
        offset: 0xcac0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xcac4,
        mask: 0xffffffff,
        value: 0x02480080,
    },
    Tunable {
        offset: 0xcb00,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xcb04,
        mask: 0xffffffff,
        value: 0x02340014,
    },
    Tunable {
        offset: 0xcb40,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xcb44,
        mask: 0xffffffff,
        value: 0x03ec0014,
    },
    Tunable {
        offset: 0xcb80,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xcb84,
        mask: 0xffffffff,
        value: 0x03a4001c,
    },
    Tunable {
        offset: 0xcbc0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xcbc4,
        mask: 0xffffffff,
        value: 0x04000040,
    },
    Tunable {
        offset: 0xcc00,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xcc04,
        mask: 0xffffffff,
        value: 0x03c00040,
    },
    Tunable {
        offset: 0xcc40,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xcc44,
        mask: 0xffffffff,
        value: 0x044000c0,
    },
    Tunable {
        offset: 0xcc80,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xcc88,
        mask: 0x0fff0fff,
        value: 0x02f00060,
    },
    Tunable {
        offset: 0xcc8c,
        mask: 0x0fff0fff,
        value: 0x02c80014,
    },
    Tunable {
        offset: 0xccc0,
        mask: 0xc0000000,
        value: 0xc0000000,
    },
    Tunable {
        offset: 0xccc8,
        mask: 0x0fff0fff,
        value: 0x03500054,
    },
    Tunable {
        offset: 0xcccc,
        mask: 0x0fff0fff,
        value: 0x02dc0014,
    },
    Tunable {
        offset: 0xcd00,
        mask: 0xc0000003,
        value: 0xc0000003,
    },
];

#[cfg(feature = "v2-t0")]
const SELECTED_TUNABLES: &[Tunable] = V2_T0_TUNABLES;
#[cfg(not(feature = "v2-t0"))]
const SELECTED_TUNABLES: &[Tunable] = &[];

/// Return the compile-time-selected AVD firmware variant.
///
/// # Returns
///
/// Selected variant descriptor.
pub const fn selected_variant() -> FirmwareVariant {
    FirmwareVariant {
        name: SELECTED_VARIANT_NAME,
        decode_ctrl_base: DECODE_CTRL_BASE,
        tunables: SELECTED_TUNABLES,
    }
}

/// Apply tunables for the selected firmware variant.
pub fn apply_selected_tunables() {
    let variant = selected_variant();
    let _ = variant.name;
    for tunable in variant.tunables {
        let ptr = (variant.decode_ctrl_base + tunable.offset as usize) as *mut u32;
        // SAFETY: Tunable offsets are version-specific CM3-visible AVD MMIO addresses.
        unsafe {
            let old_value = core::ptr::read_volatile(ptr);
            let new_value = (old_value & !tunable.mask) | tunable.value;
            core::ptr::write_volatile(ptr, new_value);
        }
    }
}
