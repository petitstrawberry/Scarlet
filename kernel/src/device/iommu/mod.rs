//! Generic IOMMU abstractions for DMA-capable kernel device drivers.
//!
//! This module defines provider-neutral IOMMU controllers, domains, firmware
//! specifiers, and DMA mapping context helpers used by platform and PCI drivers.

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::{BitOr, BitOrAssign};

/// Physical address type used by DMA mappings.
pub type PhysAddr = usize;

/// I/O virtual address type used by IOMMU domains.
pub type Iova = u64;

/// DMA address returned to device drivers.
pub type DmaAddr = u64;

/// IOMMU operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IommuError {
    /// Firmware specifier cells are malformed or invalid.
    InvalidSpec,
    /// Referenced IOMMU controller was not found.
    ControllerNotFound,
    /// Domain allocation failed.
    DomainAllocationFailed,
    /// Stream attachment failed.
    AttachFailed,
    /// Mapping operation failed.
    MapFailed,
    /// Unmapping operation failed.
    UnmapFailed,
    /// No IOVA space is available.
    OutOfIova,
    /// Operation is not supported by this controller or domain.
    NotSupported,
    /// Resource is busy and cannot satisfy the operation.
    Busy,
}

/// IOMMU mapping permission and behavior flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IommuMapFlags(u32);

impl IommuMapFlags {
    /// Mapping permits device reads.
    pub const READ: Self = Self(1 << 0);
    /// Mapping permits device writes.
    pub const WRITE: Self = Self(1 << 1);
    /// Mapping permits instruction fetches.
    pub const EXECUTE: Self = Self(1 << 2);
    /// Mapping is cache coherent with the CPU.
    pub const COHERENT: Self = Self(1 << 3);

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

impl BitOr for IommuMapFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for IommuMapFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// IOMMU stream identifier for a DMA-capable requester.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct IommuStreamId {
    /// Main stream identifier.
    pub id: u32,
    /// Optional substream identifier when supported by the controller.
    pub substream_id: Option<u32>,
}

/// Firmware IOMMU specifier for a device requester.
#[derive(Debug, Clone)]
pub struct IommuSpec {
    /// Firmware phandle identifying the IOMMU controller node.
    pub controller_phandle: u32,
    /// Provider-specific specifier cells after the controller phandle.
    pub cells: Vec<u32>,
}

/// Type of IOMMU domain to allocate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IommuDomainType {
    /// Translated DMA domain for normal DMA mappings.
    Dma,
    /// Identity-mapped domain for devices that need physical-address IOVA.
    Identity,
}

/// Configuration used when allocating an IOMMU domain.
#[derive(Debug, Clone, Copy)]
pub struct IommuDomainConfig {
    /// Requested domain type.
    pub domain_type: IommuDomainType,
    /// Base IOVA available to the domain.
    pub iova_base: Iova,
    /// Size of the IOVA aperture in bytes.
    pub iova_size: u64,
}

/// IOMMU translation domain.
pub trait IommuDomain: Send + Sync {
    /// Attach a requester stream to this domain.
    ///
    /// # Arguments
    ///
    /// * `stream` - Stream identifier to attach.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the stream is attached.
    fn attach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError>;

    /// Detach a requester stream from this domain.
    ///
    /// # Arguments
    ///
    /// * `stream` - Stream identifier to detach.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the stream is detached.
    fn detach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError>;

    /// Map a physical memory range into the domain.
    ///
    /// # Arguments
    ///
    /// * `iova` - I/O virtual address to map.
    /// * `paddr` - Physical address backing the mapping.
    /// * `len` - Mapping length in bytes.
    /// * `flags` - Mapping permissions and behavior flags.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the mapping is installed.
    fn map(
        &self,
        iova: Iova,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<(), IommuError>;

    /// Remove a mapping from the domain.
    ///
    /// # Arguments
    ///
    /// * `iova` - I/O virtual address of the mapping to remove.
    /// * `len` - Mapping length in bytes.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the mapping is removed.
    fn unmap(&self, iova: Iova, len: usize) -> Result<(), IommuError>;

    /// Translate an IOVA back to a physical address when possible.
    ///
    /// # Arguments
    ///
    /// * `iova` - I/O virtual address to translate.
    ///
    /// # Returns
    ///
    /// Physical address backing `iova`, or `None` when unmapped.
    fn iova_to_phys(&self, iova: Iova) -> Option<PhysAddr>;

    /// Flush pending IOMMU translation updates.
    ///
    /// # Returns
    ///
    /// `Ok(())` when hardware-visible state is synchronized.
    fn flush(&self) -> Result<(), IommuError>;
}

/// IOMMU controller registered by firmware phandle.
pub trait IommuController: Send + Sync {
    /// Return the controller name.
    ///
    /// # Returns
    ///
    /// Static controller name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Allocate an IOMMU domain.
    ///
    /// # Arguments
    ///
    /// * `config` - Domain configuration requested by the caller.
    ///
    /// # Returns
    ///
    /// A reference-counted domain on success.
    fn alloc_domain(&self, config: IommuDomainConfig) -> Result<Arc<dyn IommuDomain>, IommuError>;

    /// Decode firmware specifier cells into stream IDs.
    ///
    /// # Arguments
    ///
    /// * `spec` - Firmware IOMMU specifier for this controller.
    ///
    /// # Returns
    ///
    /// Stream IDs represented by `spec`.
    fn stream_ids_from_fdt(&self, spec: &IommuSpec) -> Result<Vec<IommuStreamId>, IommuError>;
}

/// Resolved IOMMU attachment for a device.
#[derive(Clone)]
pub struct IommuAttachment {
    /// IOMMU controller used by the attachment.
    pub controller: Arc<dyn IommuController>,
    /// IOMMU domain allocated for the device.
    pub domain: Arc<dyn IommuDomain>,
    /// Streams attached to the domain.
    pub streams: Vec<IommuStreamId>,
}

/// DMA mapping context for a device.
#[derive(Clone)]
pub struct DmaContext {
    /// Optional IOMMU attachment. `None` means direct DMA.
    pub iommu: Option<IommuAttachment>,
    /// Offset applied to physical addresses for direct DMA.
    pub direct_dma_offset: isize,
}

impl DmaContext {
    /// Create a direct-DMA context without an IOMMU attachment.
    ///
    /// # Returns
    ///
    /// A DMA context that maps physical addresses directly.
    pub fn direct() -> Self {
        Self {
            iommu: None,
            direct_dma_offset: 0,
        }
    }

    /// Map a physical memory range for device DMA.
    ///
    /// # Arguments
    ///
    /// * `paddr` - Physical address backing the mapping.
    /// * `len` - Mapping length in bytes.
    /// * `flags` - Mapping permissions and behavior flags.
    ///
    /// # Returns
    ///
    /// DMA address to program into the device.
    pub fn map_phys(
        &self,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<DmaAddr, IommuError> {
        if let Some(attachment) = &self.iommu {
            let iova = paddr as Iova;
            attachment.domain.map(iova, paddr, len, flags)?;
            Ok(iova)
        } else {
            Ok((paddr as isize + self.direct_dma_offset) as DmaAddr)
        }
    }

    /// Unmap a DMA address previously returned by [`Self::map_phys`].
    ///
    /// # Arguments
    ///
    /// * `dma_addr` - DMA address to unmap.
    /// * `len` - Mapping length in bytes.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the mapping is removed or direct DMA needs no action.
    pub fn unmap(&self, dma_addr: DmaAddr, len: usize) -> Result<(), IommuError> {
        if let Some(attachment) = &self.iommu {
            attachment.domain.unmap(dma_addr as Iova, len)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spin::Mutex;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct RecordedMap {
        iova: Iova,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    }

    struct TestDomain {
        last_map: Mutex<Option<RecordedMap>>,
    }

    impl TestDomain {
        fn new() -> Self {
            Self {
                last_map: Mutex::new(None),
            }
        }

        fn last_map(&self) -> Option<RecordedMap> {
            *self.last_map.lock()
        }
    }

    impl IommuDomain for TestDomain {
        fn attach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
            let _ = stream;
            Ok(())
        }

        fn detach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
            let _ = stream;
            Ok(())
        }

        fn map(
            &self,
            iova: Iova,
            paddr: PhysAddr,
            len: usize,
            flags: IommuMapFlags,
        ) -> Result<(), IommuError> {
            *self.last_map.lock() = Some(RecordedMap {
                iova,
                paddr,
                len,
                flags,
            });
            Ok(())
        }

        fn unmap(&self, iova: Iova, len: usize) -> Result<(), IommuError> {
            let _ = (iova, len);
            Ok(())
        }

        fn iova_to_phys(&self, iova: Iova) -> Option<PhysAddr> {
            let _ = iova;
            None
        }

        fn flush(&self) -> Result<(), IommuError> {
            Ok(())
        }
    }

    struct TestController {
        domain: Arc<TestDomain>,
    }

    impl IommuController for TestController {
        fn name(&self) -> &'static str {
            "test-iommu"
        }

        fn alloc_domain(
            &self,
            config: IommuDomainConfig,
        ) -> Result<Arc<dyn IommuDomain>, IommuError> {
            let _ = config;
            Ok(self.domain.clone())
        }

        fn stream_ids_from_fdt(&self, spec: &IommuSpec) -> Result<Vec<IommuStreamId>, IommuError> {
            let _ = spec;
            Ok(Vec::new())
        }
    }

    #[test_case]
    fn test_iommu_map_flags_contains_and_bitor() {
        let mut flags = IommuMapFlags::READ | IommuMapFlags::WRITE;
        flags |= IommuMapFlags::COHERENT;
        assert!(flags.contains(IommuMapFlags::READ));
        assert!(flags.contains(IommuMapFlags::WRITE));
        assert!(flags.contains(IommuMapFlags::COHERENT));
        assert!(!flags.contains(IommuMapFlags::EXECUTE));
        assert_eq!(flags.bits(), 0b1011);
    }

    #[test_case]
    fn test_iommu_stream_id_ordering() {
        let lower = IommuStreamId {
            id: 1,
            substream_id: None,
        };
        let higher = IommuStreamId {
            id: 1,
            substream_id: Some(1),
        };
        assert!(lower < higher);
    }

    #[test_case]
    fn test_dma_context_direct_map_phys() {
        let context = DmaContext::direct();
        assert_eq!(
            context
                .map_phys(0x1000, 0x100, IommuMapFlags::READ)
                .unwrap(),
            0x1000
        );
    }

    #[test_case]
    fn test_dma_context_iommu_map_phys_uses_identity_iova() {
        let domain = Arc::new(TestDomain::new());
        let controller = Arc::new(TestController {
            domain: domain.clone(),
        });
        let context = DmaContext {
            iommu: Some(IommuAttachment {
                controller,
                domain: domain.clone(),
                streams: Vec::new(),
            }),
            direct_dma_offset: 0,
        };

        let dma_addr = context
            .map_phys(0x2000, 0x200, IommuMapFlags::READ | IommuMapFlags::WRITE)
            .unwrap();
        assert_eq!(dma_addr, 0x2000);
        assert_eq!(
            domain.last_map(),
            Some(RecordedMap {
                iova: 0x2000,
                paddr: 0x2000,
                len: 0x200,
                flags: IommuMapFlags::READ | IommuMapFlags::WRITE,
            })
        );
    }

    #[test_case]
    fn test_dma_context_unmap_passthrough_when_no_iommu() {
        let context = DmaContext::direct();
        assert_eq!(context.unmap(0x1000, 0x100), Ok(()));
    }
}
