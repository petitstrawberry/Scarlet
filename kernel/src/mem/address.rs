//! Address values, independent of the CPU's pointer width and of mappings.
//!
//! Physical, DMA and I/O virtual addresses retain their full 64-bit range.
//! Only a CPU virtual address has pointer width. Constructing one of these
//! values grants neither access to memory nor ownership of a mapping.

macro_rules! address_value {
    ($name:ident, $word:ty, $accessor:ident, $description:literal) => {
        #[doc = $description]
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name($word);

        impl $name {
            /// The address whose numeric value is zero.
            pub const ZERO: Self = Self(0);

            /// Preserve a raw address without claiming it is mapped or supported by hardware.
            pub const fn new(address: $word) -> Self {
                Self(address)
            }

            /// Extract the address at an explicit encoding or translation boundary.
            pub const fn $accessor(self) -> $word {
                self.0
            }

            pub const fn is_zero(self) -> bool {
                self.0 == 0
            }

            /// Advance by a byte offset, rejecting overflow.
            pub const fn checked_add(self, bytes: $word) -> Option<Self> {
                match self.0.checked_add(bytes) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }

            /// Move back by a byte offset, rejecting underflow.
            pub const fn checked_sub(self, bytes: $word) -> Option<Self> {
                match self.0.checked_sub(bytes) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }

            /// Measure a nonnegative byte offset within the same address space.
            pub const fn checked_offset_from(self, base: Self) -> Option<$word> {
                self.0.checked_sub(base.0)
            }

            /// Test a power-of-two alignment; invalid alignments return false.
            pub const fn is_aligned(self, alignment: $word) -> bool {
                alignment.is_power_of_two() && self.0 & (alignment - 1) == 0
            }

            /// Round down, rejecting zero and non-power-of-two alignments.
            pub const fn checked_align_down(self, alignment: $word) -> Option<Self> {
                if !alignment.is_power_of_two() {
                    return None;
                }
                Some(Self(self.0 & !(alignment - 1)))
            }

            /// Round up, rejecting invalid alignments and address-space overflow.
            pub const fn checked_align_up(self, alignment: $word) -> Option<Self> {
                if !alignment.is_power_of_two() {
                    return None;
                }
                match self.0.checked_add(alignment - 1) {
                    Some(value) => Some(Self(value & !(alignment - 1))),
                    None => None,
                }
            }
        }

        impl core::fmt::LowerHex for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::LowerHex::fmt(&self.0, f)
            }
        }

        impl core::fmt::UpperHex for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::UpperHex::fmt(&self.0, f)
            }
        }
    };
}

address_value!(
    PhysAddr,
    u64,
    as_u64,
    "An address in the CPU physical address space."
);
address_value!(
    VirtAddr,
    usize,
    as_usize,
    "An address in a CPU virtual address space."
);
address_value!(
    DmaAddr,
    u64,
    as_u64,
    "An address visible to a DMA requester."
);
address_value!(
    Iova,
    u64,
    as_u64,
    "An address in an IOMMU domain's input address space."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn physical_and_device_addresses_preserve_bits_above_32_bits() {
        let physical = PhysAddr::new(0x1_8000_0123);
        assert_eq!(physical.as_u64(), 0x1_8000_0123);
        assert_eq!(
            physical.checked_align_down(4096),
            Some(PhysAddr::new(0x1_8000_0000))
        );
        assert_eq!(
            physical.checked_align_up(4096),
            Some(PhysAddr::new(0x1_8000_1000))
        );
        assert_eq!(DmaAddr::new(0x1_8000_0123).as_u64(), 0x1_8000_0123);
        assert_eq!(Iova::new(0x2_8000_0123).as_u64(), 0x2_8000_0123);
    }

    #[test_case]
    fn address_arithmetic_rejects_overflow_and_invalid_alignment() {
        assert_eq!(PhysAddr::new(u64::MAX).checked_add(1), None);
        assert_eq!(PhysAddr::ZERO.checked_sub(1), None);
        assert_eq!(PhysAddr::new(u64::MAX).checked_align_up(4096), None);
        assert_eq!(
            PhysAddr::new(u64::MAX).checked_align_up(1),
            Some(PhysAddr::new(u64::MAX))
        );
        assert_eq!(PhysAddr::ZERO.checked_align_up(0), None);
        assert_eq!(PhysAddr::new(123).checked_align_down(3), None);
        assert!(!PhysAddr::ZERO.is_aligned(0));
        assert_eq!(VirtAddr::new(usize::MAX).checked_add(1), None);
        assert_eq!(VirtAddr::new(usize::MAX).checked_align_up(4096), None);
        assert_eq!(DmaAddr::ZERO.checked_offset_from(DmaAddr::new(1)), None);
    }
}
