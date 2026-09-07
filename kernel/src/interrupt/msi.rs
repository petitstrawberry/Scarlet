//! Message Signaled Interrupt controller abstractions.
//!
//! MSI controllers allocate virtual IRQ mappings and return the doorbell message
//! data that PCI MSI/MSI-X requesters must program into their capability tables.

extern crate alloc;

use alloc::vec::Vec;
use core::ops::{BitOr, BitOrAssign};

use crate::interrupt::{CpuId, Hwirq, Virq};

/// MSI operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsiError {
    /// Referenced MSI controller was not found.
    ControllerNotFound,
    /// No vectors are available for the request.
    NoVectors,
    /// Request parameters are invalid.
    InvalidRequest,
    /// Operation is not supported by this controller.
    NotSupported,
    /// Hardware access failed.
    HardwareError,
    /// Controller or vector is busy.
    Busy,
}

/// PCI requester identity used by MSI/MSI-X routing domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsiRequester {
    /// PCI segment/domain.
    pub segment: u16,
    /// PCI bus number.
    pub bus: u8,
    /// PCI device number.
    pub device: u8,
    /// PCI function number.
    pub function: u8,
}

/// MSI allocation request behavior flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsiRequestFlags(u32);

impl MsiRequestFlags {
    /// No request flags.
    pub const NONE: Self = Self(0);
    /// Vectors must be contiguous for classic MSI programming.
    pub const CONTIGUOUS: Self = Self(1 << 0);
    /// Request is for MSI-X vectors.
    pub const MSI_X: Self = Self(1 << 1);

    /// Returns true when all bits in `other` are present.
    ///
    /// # Arguments
    ///
    /// * `other` - Flags that must be contained in `self`.
    ///
    /// # Returns
    ///
    /// `true` if every bit from `other` is set in `self`.
    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Return the raw flag bits.
    ///
    /// # Returns
    ///
    /// Raw `u32` representation of the flags.
    pub fn bits(self) -> u32 {
        self.0
    }
}

impl BitOr for MsiRequestFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for MsiRequestFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// MSI/MSI-X allocation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsiRequest {
    /// Number of vectors requested.
    pub count: usize,
    /// Preferred target CPU.
    pub target_cpu: CpuId,
    /// Optional requester identity for routing domains that need requester IDs.
    pub requester: Option<MsiRequester>,
    /// Allocation behavior flags.
    pub flags: MsiRequestFlags,
}

/// Message written by a PCI MSI/MSI-X requester to raise an interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsiMessage {
    /// MSI doorbell address.
    pub address: u64,
    /// MSI message data payload.
    pub data: u32,
}

/// One interrupt vector allocated for MSI/MSI-X.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsiVector {
    /// Kernel virtual IRQ associated with this vector.
    pub virq: Virq,
    /// Controller-local hardware IRQ associated with this vector.
    pub hwirq: Hwirq,
    /// Doorbell message programmed into the PCI device.
    pub message: MsiMessage,
}

/// A contiguous or controller-defined MSI/MSI-X allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsiAllocation {
    /// Allocated vectors.
    pub vectors: Vec<MsiVector>,
}

/// MSI controller registered by firmware phandle.
pub trait MsiController: Send + Sync {
    /// Return the controller name.
    ///
    /// # Returns
    ///
    /// Static controller name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Allocate MSI/MSI-X vectors for a requester.
    ///
    /// # Arguments
    ///
    /// * `request` - Vector count, target CPU, requester identity, and flags.
    ///
    /// # Returns
    ///
    /// Allocated MSI vectors and message programming data.
    fn allocate_vectors(&self, request: MsiRequest) -> Result<MsiAllocation, MsiError>;

    /// Free a previous MSI/MSI-X allocation.
    ///
    /// # Arguments
    ///
    /// * `allocation` - Allocation returned by [`MsiController::allocate_vectors`].
    fn free_vectors(&self, allocation: &MsiAllocation);

    /// Mask one allocated vector.
    ///
    /// # Arguments
    ///
    /// * `vector` - Vector to mask.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the vector is masked.
    fn mask_vector(&self, vector: &MsiVector) -> Result<(), MsiError>;

    /// Unmask one allocated vector.
    ///
    /// # Arguments
    ///
    /// * `vector` - Vector to unmask.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the vector is unmasked.
    fn unmask_vector(&self, vector: &MsiVector) -> Result<(), MsiError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct FakeMsiController {
        allocated: AtomicUsize,
        freed: AtomicUsize,
        masked: AtomicBool,
    }

    impl FakeMsiController {
        fn new() -> Self {
            Self {
                allocated: AtomicUsize::new(0),
                freed: AtomicUsize::new(0),
                masked: AtomicBool::new(false),
            }
        }
    }

    impl MsiController for FakeMsiController {
        fn name(&self) -> &'static str {
            "fake-msi"
        }

        fn allocate_vectors(&self, request: MsiRequest) -> Result<MsiAllocation, MsiError> {
            if request.count == 0 {
                return Err(MsiError::InvalidRequest);
            }
            self.allocated.fetch_add(request.count, Ordering::SeqCst);
            let mut vectors = Vec::new();
            for index in 0..request.count {
                vectors.push(MsiVector {
                    virq: 32 + index as u32,
                    hwirq: 64 + index as u32,
                    message: MsiMessage {
                        address: 0xfee0_0000,
                        data: index as u32,
                    },
                });
            }
            Ok(MsiAllocation { vectors })
        }

        fn free_vectors(&self, allocation: &MsiAllocation) {
            self.freed
                .fetch_add(allocation.vectors.len(), Ordering::SeqCst);
        }

        fn mask_vector(&self, vector: &MsiVector) -> Result<(), MsiError> {
            let _ = vector;
            self.masked.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn unmask_vector(&self, vector: &MsiVector) -> Result<(), MsiError> {
            let _ = vector;
            self.masked.store(false, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test_case]
    fn test_msi_request_flags_contains_and_bitor() {
        let flags = MsiRequestFlags::CONTIGUOUS | MsiRequestFlags::MSI_X;
        assert!(flags.contains(MsiRequestFlags::CONTIGUOUS));
        assert!(flags.contains(MsiRequestFlags::MSI_X));
        assert!(flags.contains(MsiRequestFlags::NONE));
        assert_eq!(flags.bits(), 0b11);

        let mut assigned = MsiRequestFlags::NONE;
        assigned |= MsiRequestFlags::MSI_X;
        assert!(assigned.contains(MsiRequestFlags::MSI_X));
    }

    #[test_case]
    fn test_msi_controller_allocate_and_free_roundtrip() {
        let controller = FakeMsiController::new();
        let allocation = controller
            .allocate_vectors(MsiRequest {
                count: 2,
                target_cpu: 0,
                requester: None,
                flags: MsiRequestFlags::MSI_X,
            })
            .expect("expected MSI allocation");

        assert_eq!(allocation.vectors.len(), 2);
        assert_eq!(controller.allocated.load(Ordering::SeqCst), 2);
        assert_eq!(allocation.vectors[0].message.address, 0xfee0_0000);

        controller.free_vectors(&allocation);
        assert_eq!(controller.freed.load(Ordering::SeqCst), 2);
    }

    #[test_case]
    fn test_msi_controller_mask_unmask() {
        let controller = FakeMsiController::new();
        let vector = MsiVector {
            virq: 32,
            hwirq: 64,
            message: MsiMessage {
                address: 0xfee0_0000,
                data: 1,
            },
        };

        controller.mask_vector(&vector).unwrap();
        assert!(controller.masked.load(Ordering::SeqCst));
        controller.unmask_vector(&vector).unwrap();
        assert!(!controller.masked.load(Ordering::SeqCst));
    }
}
