//! Generic IOMMU abstractions for DMA-capable kernel device drivers.
//!
//! This module defines provider-neutral IOMMU controllers, domains, firmware
//! specifiers, and DMA mapping context helpers used by platform and PCI drivers.

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::{BitOr, BitOrAssign};

use crate::sync::IrqSpinLock;

pub use crate::mem::address::{DmaAddr, Iova, PhysAddr};

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
    /// Mapping permissions apply only to privileged device transactions.
    ///
    /// Requesters that issue both supervisor and user transactions use this
    /// for firmware or other supervisor-only address ranges.
    pub const PRIVILEGED: Self = Self(1 << 4);

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

/// Snapshot of an IOMMU domain's hardware fault registers.
///
/// Controllers without per-domain fault reporting may return no snapshot.
/// The values are intentionally controller-neutral raw registers so device
/// drivers can include them in failure diagnostics without depending on one
/// IOMMU implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IommuFaultSnapshot {
    /// Controller-wide fault status.
    pub global_status: u32,
    /// Domain or context-bank fault status.
    pub context_status: u32,
    /// Device-visible address associated with the fault.
    pub fault_address: u64,
    /// Primary implementation-defined fault syndrome.
    pub syndrome0: u32,
    /// Secondary implementation-defined fault syndrome.
    pub syndrome1: u32,
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

    /// Return the minimum mapping granule for this domain.
    ///
    /// # Returns
    ///
    /// Smallest byte granule that can be independently mapped and unmapped.
    fn page_size(&self) -> usize {
        crate::environment::PAGE_SIZE
    }

    /// Flush pending IOMMU translation updates.
    ///
    /// # Returns
    ///
    /// `Ok(())` when hardware-visible state is synchronized.
    fn flush(&self) -> Result<(), IommuError>;

    /// Capture the current hardware fault state for this domain.
    ///
    /// # Returns
    ///
    /// A raw controller-neutral snapshot when the implementation exposes
    /// fault registers, otherwise `None`.
    fn fault_snapshot(&self) -> Option<IommuFaultSnapshot> {
        None
    }
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
    /// Additional IOMMU attachments for devices whose DMA reaches multiple controllers.
    pub additional_iommus: Vec<IommuAttachment>,
    /// Offset applied to physical addresses for direct DMA.
    pub direct_dma_offset: i64,
    iova_allocator: Option<Arc<IrqSpinLock<DmaIovaAllocator>>>,
}

/// Owned DMA mapping that is unmapped when dropped.
pub struct DmaMapping {
    context: DmaContext,
    dma_addr: DmaAddr,
    len: usize,
    mapped: bool,
}

impl DmaMapping {
    fn new(context: DmaContext, dma_addr: DmaAddr, len: usize) -> Self {
        Self {
            context,
            dma_addr,
            len,
            mapped: true,
        }
    }

    /// Return the device-visible DMA address.
    ///
    /// # Returns
    ///
    /// DMA address to program into the device.
    pub const fn dma_addr(&self) -> DmaAddr {
        self.dma_addr
    }

    /// Return the mapped range length in bytes.
    ///
    /// # Returns
    ///
    /// Number of bytes covered by the DMA mapping.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Return whether this mapping covers no bytes.
    ///
    /// # Returns
    ///
    /// `true` when the mapping length is zero.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Explicitly unmap this DMA mapping.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the mapping is removed or was already inactive.
    pub fn unmap(mut self) -> Result<(), IommuError> {
        if self.mapped {
            let result = self.context.unmap(self.dma_addr, self.len);
            if result.is_ok() {
                self.mapped = false;
            }
            result
        } else {
            Ok(())
        }
    }
}

impl Drop for DmaMapping {
    fn drop(&mut self) {
        if self.mapped {
            let _ = self.context.unmap(self.dma_addr, self.len);
            self.mapped = false;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DmaIovaRange {
    start: Iova,
    len: u64,
}

impl DmaIovaRange {
    fn last_address(self) -> Option<Iova> {
        self.start.checked_add(self.len.checked_sub(1)?)
    }
}

struct DmaIovaAllocator {
    free: Vec<DmaIovaRange>,
}

impl DmaIovaAllocator {
    fn new(base: Iova, size: u64) -> Self {
        Self {
            free: alloc::vec![DmaIovaRange {
                start: base,
                len: size,
            }],
        }
    }

    fn alloc(&mut self, len: u64, align: u64) -> Result<Iova, IommuError> {
        if len == 0 || align == 0 {
            return Err(IommuError::OutOfIova);
        }

        for index in 0..self.free.len() {
            let range = self.free[index];
            if range.last_address().is_none() {
                continue;
            }
            let Some(start) = align_up_u64(range.start.as_u64(), align).map(Iova::new) else {
                continue;
            };
            let padding = start
                .checked_offset_from(range.start)
                .expect("aligned IOVA precedes free range");
            if padding > range.len {
                continue;
            }
            let available = range.len - padding;
            if available < len {
                continue;
            }

            let before = padding;
            let after_len = available - len;
            match (before, after_len) {
                (0, 0) => {
                    self.free.remove(index);
                }
                (0, _) => {
                    self.free[index] = DmaIovaRange {
                        start: start.checked_add(len).expect("validated IOVA tail"),
                        len: after_len,
                    };
                }
                (_, 0) => {
                    self.free[index].len = before;
                }
                (_, _) => {
                    self.free[index].len = before;
                    self.free.insert(
                        index + 1,
                        DmaIovaRange {
                            start: start.checked_add(len).expect("validated IOVA tail"),
                            len: after_len,
                        },
                    );
                }
            }
            return Ok(start);
        }

        Err(IommuError::OutOfIova)
    }

    fn free(&mut self, start: Iova, len: u64) {
        if len == 0 {
            return;
        }

        self.free.push(DmaIovaRange { start, len });
        self.free.sort_by_key(|range| range.start);

        let mut merged: Vec<DmaIovaRange> = Vec::new();
        for range in self.free.drain(..) {
            if let Some(last) = merged.last_mut() {
                let last_end = last.last_address().expect("valid free IOVA range");
                if range.start <= last_end || last_end.checked_add(1) == Some(range.start) {
                    let range_end = range.last_address().expect("valid returned IOVA range");
                    last.len = range_end
                        .max(last_end)
                        .checked_offset_from(last.start)
                        .and_then(|offset| offset.checked_add(1))
                        .expect("merged free range fits its IOVA aperture");
                    continue;
                }
            }
            merged.push(range);
        }
        self.free = merged;
    }
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
            additional_iommus: Vec::new(),
            direct_dma_offset: 0,
            iova_allocator: None,
        }
    }

    /// Create a DMA context from resolved IOMMU attachments.
    ///
    /// # Arguments
    ///
    /// * `iommu` - Primary IOMMU attachment, if any.
    /// * `additional_iommus` - Additional IOMMU attachments reached by the device.
    /// * `config` - Domain configuration used to allocate translated IOVA space.
    ///
    /// # Returns
    ///
    /// A DMA context that returns allocated IOVA addresses when `config.iova_base`
    /// is non-zero, or preserves physical-address IOVA behavior otherwise.
    pub fn from_iommu_attachments(
        iommu: Option<IommuAttachment>,
        additional_iommus: Vec<IommuAttachment>,
        config: IommuDomainConfig,
    ) -> Self {
        let iova_allocator =
            if iommu.is_some() && !config.iova_base.is_zero() && config.iova_size != 0 {
                Some(Arc::new(IrqSpinLock::new(DmaIovaAllocator::new(
                    config.iova_base,
                    config.iova_size,
                ))))
            } else {
                None
            };
        Self {
            iommu,
            additional_iommus,
            direct_dma_offset: 0,
            iova_allocator,
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
        self.map_phys_internal(paddr, len, flags)
            .map(|(dma_addr, _mapped_len)| dma_addr)
    }

    fn map_phys_internal(
        &self,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<(DmaAddr, usize), IommuError> {
        let last_offset = len.checked_sub(1).ok_or(IommuError::MapFailed)? as u64;
        paddr
            .checked_add(last_offset)
            .ok_or(IommuError::MapFailed)?;
        if let Some(attachment) = &self.iommu {
            let mapped_len = self.mapped_len(len)?;
            paddr
                .checked_add(mapped_len as u64 - 1)
                .ok_or(IommuError::MapFailed)?;
            let iova = self.alloc_iova(paddr, mapped_len)?;
            if let Err(error) = attachment.domain.map(iova, paddr, mapped_len, flags) {
                self.free_iova(iova, mapped_len);
                return Err(error);
            }
            let mut mapped_additionals = 0;
            for additional in &self.additional_iommus {
                if let Err(error) = additional.domain.map(iova, paddr, mapped_len, flags) {
                    for mapped in self.additional_iommus.iter().take(mapped_additionals).rev() {
                        let _ = mapped.domain.unmap(iova, mapped_len);
                    }
                    let _ = attachment.domain.unmap(iova, mapped_len);
                    self.free_iova(iova, mapped_len);
                    return Err(error);
                }
                mapped_additionals += 1;
            }
            Ok((DmaAddr::new(iova.as_u64()), mapped_len))
        } else {
            let dma_addr = DmaAddr::new(
                paddr
                    .as_u64()
                    .checked_add_signed(self.direct_dma_offset)
                    .ok_or(IommuError::MapFailed)?,
            );
            dma_addr
                .checked_add(last_offset)
                .ok_or(IommuError::MapFailed)?;
            Ok((dma_addr, len))
        }
    }

    /// Return the strictest mapping granule required by attached IOMMUs.
    ///
    /// # Returns
    ///
    /// DMA mappings should be aligned to this size and cover a multiple of it
    /// to avoid sharing an IOMMU PTE with unrelated DMA objects.
    pub fn mapping_granule(&self) -> usize {
        let mut granule = crate::environment::PAGE_SIZE;
        if let Some(attachment) = &self.iommu {
            granule = granule.max(attachment.domain.page_size());
        }
        for attachment in &self.additional_iommus {
            granule = granule.max(attachment.domain.page_size());
        }
        granule
    }

    /// Capture the primary IOMMU domain's current fault state.
    ///
    /// # Returns
    ///
    /// A hardware fault snapshot for translated DMA, or `None` for direct DMA
    /// and controllers without diagnostic registers.
    pub fn primary_iommu_fault_snapshot(&self) -> Option<IommuFaultSnapshot> {
        self.iommu.as_ref()?.domain.fault_snapshot()
    }

    /// Restore IOMMU stream programming after a device power-domain reset.
    ///
    /// Page tables and DMA mappings remain owned by their domains, but a reset
    /// may clear controller-side stream registers such as TTBRs and TCRs. This
    /// reattaches every resolved stream and flushes the restored domains.
    ///
    /// # Returns
    ///
    /// `Ok(())` when all stream attachments and domain flushes completed.
    pub fn restore_iommu(&self) -> Result<(), IommuError> {
        if let Some(attachment) = &self.iommu {
            for stream in &attachment.streams {
                attachment.domain.attach_stream(*stream)?;
            }
            attachment.domain.flush()?;
        }
        for attachment in &self.additional_iommus {
            for stream in &attachment.streams {
                attachment.domain.attach_stream(*stream)?;
            }
            attachment.domain.flush()?;
        }
        Ok(())
    }

    /// Map a physical memory range and return an owned DMA mapping.
    ///
    /// The returned mapping automatically unmaps the DMA address when dropped.
    /// Use this for short-lived transfer buffers; long-lived rings and device
    /// contexts should keep using [`Self::map_phys`] and unmap during teardown.
    ///
    /// # Arguments
    ///
    /// * `paddr` - Physical address backing the mapping.
    /// * `len` - Mapping length in bytes.
    /// * `flags` - Mapping permissions and behavior flags.
    ///
    /// # Returns
    ///
    /// Owned DMA mapping to program into the device.
    pub fn map_phys_owned(
        &self,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<DmaMapping, IommuError> {
        let (dma_addr, mapped_len) = self.map_phys_internal(paddr, len, flags)?;
        Ok(DmaMapping::new(self.clone(), dma_addr, mapped_len))
    }

    /// Map discontiguous physical segments into one contiguous DMA address range.
    ///
    /// This is the scatter/gather counterpart to [`Self::map_phys_owned`]. Each
    /// segment must be aligned to the strictest attached-IOMMU granule. A
    /// translated context with an IOVA allocator is required unless the
    /// supplied segments are already physically contiguous.
    ///
    /// # Arguments
    ///
    /// * `segments` - Physical base and byte length for each backing segment.
    /// * `flags` - Mapping permissions and behavior flags.
    ///
    /// # Returns
    ///
    /// One owned mapping whose DMA address spans every segment in order.
    pub fn map_phys_segments_owned(
        &self,
        segments: &[(PhysAddr, usize)],
        flags: IommuMapFlags,
    ) -> Result<DmaMapping, IommuError> {
        let Some(&(first_paddr, _)) = segments.first() else {
            return Err(IommuError::MapFailed);
        };
        let granule = self.mapping_granule();
        let mut total_len = 0usize;
        let mut expected_paddr = Some(first_paddr);
        let mut physically_contiguous = true;
        for &(paddr, len) in segments {
            if len == 0 || paddr.as_u64() % granule as u64 != 0 || len % granule != 0 {
                return Err(IommuError::MapFailed);
            }
            if Some(paddr) != expected_paddr {
                physically_contiguous = false;
            }
            total_len = total_len.checked_add(len).ok_or(IommuError::MapFailed)?;
            let last = paddr
                .checked_add(len as u64 - 1)
                .ok_or(IommuError::MapFailed)?;
            expected_paddr = last.checked_add(1);
        }

        if self.iova_allocator.is_none() {
            if physically_contiguous {
                return self.map_phys_owned(first_paddr, total_len, flags);
            }
            return Err(IommuError::NotSupported);
        }

        let attachment = self.iommu.as_ref().ok_or(IommuError::NotSupported)?;
        let iova = self.alloc_iova(first_paddr, total_len)?;

        let map_segments = |target: &IommuAttachment| -> Result<(), IommuError> {
            let mut offset = 0usize;
            for &(paddr, len) in segments {
                if let Err(error) = target.domain.map(
                    iova.checked_add(offset as u64)
                        .expect("allocated IOVA range contains every segment"),
                    paddr,
                    len,
                    flags,
                ) {
                    if offset != 0 {
                        let _ = target.domain.unmap(iova, offset);
                    }
                    return Err(error);
                }
                offset += len;
            }
            Ok(())
        };

        if let Err(error) = map_segments(attachment) {
            self.free_iova(iova, total_len);
            return Err(error);
        }
        let mut mapped_additionals = 0usize;
        for additional in &self.additional_iommus {
            if let Err(error) = map_segments(additional) {
                for mapped in self.additional_iommus.iter().take(mapped_additionals).rev() {
                    let _ = mapped.domain.unmap(iova, total_len);
                }
                let _ = attachment.domain.unmap(iova, total_len);
                self.free_iova(iova, total_len);
                return Err(error);
            }
            mapped_additionals += 1;
        }

        Ok(DmaMapping::new(
            self.clone(),
            DmaAddr::new(iova.as_u64()),
            total_len,
        ))
    }

    /// Map physical memory at a caller-selected device address.
    ///
    /// This is intended for firmware ABI windows whose device address is fixed
    /// by hardware. Dynamically allocated DMA contexts reject the request so a
    /// fixed range cannot overlap allocator-owned IOVA space.
    ///
    /// # Arguments
    ///
    /// * `dma_addr` - Exact device-visible address required by the hardware ABI.
    /// * `paddr` - Physical address backing the mapping.
    /// * `len` - Mapping length in bytes.
    /// * `flags` - Mapping permissions and behavior flags.
    ///
    /// # Returns
    ///
    /// Owned mapping which removes the fixed mapping when dropped.
    pub fn map_phys_at_owned(
        &self,
        dma_addr: DmaAddr,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<DmaMapping, IommuError> {
        let last_offset = len.checked_sub(1).ok_or(IommuError::MapFailed)? as u64;
        paddr
            .checked_add(last_offset)
            .ok_or(IommuError::MapFailed)?;
        dma_addr
            .checked_add(last_offset)
            .ok_or(IommuError::MapFailed)?;
        if self.iova_allocator.is_some() {
            return Err(IommuError::Busy);
        }

        if let Some(attachment) = &self.iommu {
            attachment
                .domain
                .map(Iova::new(dma_addr.as_u64()), paddr, len, flags)?;
            let mut mapped_additionals = 0usize;
            for additional in &self.additional_iommus {
                if let Err(error) =
                    additional
                        .domain
                        .map(Iova::new(dma_addr.as_u64()), paddr, len, flags)
                {
                    for mapped in self.additional_iommus.iter().take(mapped_additionals).rev() {
                        let _ = mapped.domain.unmap(Iova::new(dma_addr.as_u64()), len);
                    }
                    let _ = attachment.domain.unmap(Iova::new(dma_addr.as_u64()), len);
                    return Err(error);
                }
                mapped_additionals += 1;
            }
        } else {
            let direct = paddr
                .as_u64()
                .checked_add_signed(self.direct_dma_offset)
                .ok_or(IommuError::MapFailed)?;
            if direct != dma_addr.as_u64() {
                return Err(IommuError::NotSupported);
            }
        }

        Ok(DmaMapping::new(self.clone(), dma_addr, len))
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
            let mapped_len = self.mapped_len(len)?;
            attachment
                .domain
                .unmap(Iova::new(dma_addr.as_u64()), mapped_len)?;
            for additional in &self.additional_iommus {
                additional
                    .domain
                    .unmap(Iova::new(dma_addr.as_u64()), mapped_len)?;
            }
            self.free_iova(Iova::new(dma_addr.as_u64()), mapped_len);
            Ok(())
        } else {
            Ok(())
        }
    }

    fn mapped_len(&self, len: usize) -> Result<usize, IommuError> {
        if self.iova_allocator.is_some() {
            align_up_usize(len, self.mapping_granule()).ok_or(IommuError::OutOfIova)
        } else {
            Ok(len)
        }
    }

    fn alloc_iova(&self, paddr: PhysAddr, len: usize) -> Result<Iova, IommuError> {
        if let Some(allocator) = &self.iova_allocator {
            allocator
                .lock()
                .alloc(len as u64, self.mapping_granule() as u64)
        } else {
            Ok(Iova::new(paddr.as_u64()))
        }
    }

    fn free_iova(&self, iova: Iova, len: usize) {
        if let Some(allocator) = &self.iova_allocator {
            allocator.lock().free(iova, len as u64);
        }
    }
}

fn align_up_usize(value: usize, align: usize) -> Option<usize> {
    if align == 0 {
        return None;
    }
    let rem = value % align;
    if rem == 0 {
        Some(value)
    } else {
        value.checked_add(align - rem)
    }
}

fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    if align == 0 {
        return None;
    }
    let rem = value % align;
    if rem == 0 {
        Some(value)
    } else {
        value.checked_add(align - rem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::IrqSpinLock;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct RecordedMap {
        iova: Iova,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    }

    struct TestDomain {
        maps: IrqSpinLock<Vec<RecordedMap>>,
        last_unmap: IrqSpinLock<Option<(Iova, usize)>>,
        unmap_count: IrqSpinLock<usize>,
        page_size: usize,
    }

    impl TestDomain {
        fn new() -> Self {
            Self::with_page_size(crate::environment::PAGE_SIZE)
        }

        fn with_page_size(page_size: usize) -> Self {
            Self {
                maps: IrqSpinLock::new(Vec::new()),
                last_unmap: IrqSpinLock::new(None),
                unmap_count: IrqSpinLock::new(0),
                page_size,
            }
        }

        fn last_map(&self) -> Option<RecordedMap> {
            self.maps.lock().last().copied()
        }

        fn maps(&self) -> Vec<RecordedMap> {
            self.maps.lock().clone()
        }

        fn last_unmap(&self) -> Option<(Iova, usize)> {
            *self.last_unmap.lock()
        }

        fn unmap_count(&self) -> usize {
            *self.unmap_count.lock()
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
            self.maps.lock().push(RecordedMap {
                iova,
                paddr,
                len,
                flags,
            });
            Ok(())
        }

        fn unmap(&self, iova: Iova, len: usize) -> Result<(), IommuError> {
            *self.last_unmap.lock() = Some((iova, len));
            *self.unmap_count.lock() += 1;
            Ok(())
        }

        fn iova_to_phys(&self, iova: Iova) -> Option<PhysAddr> {
            let _ = iova;
            None
        }

        fn page_size(&self) -> usize {
            self.page_size
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

    fn identity_iova_config() -> IommuDomainConfig {
        IommuDomainConfig {
            domain_type: IommuDomainType::Dma,
            iova_base: Iova::new(0),
            iova_size: 0,
        }
    }

    fn allocated_iova_config() -> IommuDomainConfig {
        IommuDomainConfig {
            domain_type: IommuDomainType::Dma,
            iova_base: Iova::new(0x4000_0000),
            iova_size: 0x1_0000,
        }
    }

    #[test_case]
    fn test_iommu_map_flags_contains_and_bitor() {
        let mut flags = IommuMapFlags::READ | IommuMapFlags::WRITE;
        flags |= IommuMapFlags::COHERENT;
        flags |= IommuMapFlags::PRIVILEGED;
        assert!(flags.contains(IommuMapFlags::READ));
        assert!(flags.contains(IommuMapFlags::WRITE));
        assert!(flags.contains(IommuMapFlags::COHERENT));
        assert!(flags.contains(IommuMapFlags::PRIVILEGED));
        assert!(!flags.contains(IommuMapFlags::EXECUTE));
        assert_eq!(flags.bits(), 0b1_1011);
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
    fn direct_dma_preserves_wide_addresses_and_checks_entire_ranges() {
        let mut context = DmaContext::direct();
        let flags = IommuMapFlags::READ;
        let last_page = PhysAddr::new(u64::MAX - 0xfff);
        let mapping = context
            .map_phys_segments_owned(&[(last_page, 0x1000)], flags)
            .unwrap();
        assert_eq!(mapping.dma_addr(), DmaAddr::new(last_page.as_u64()));
        context.direct_dma_offset = 0x1000;
        assert_eq!(
            context.map_phys(PhysAddr::new(0x1_8000_0000), 0x1000, flags),
            Ok(DmaAddr::new(0x1_8000_1000))
        );
        // A representable device base is insufficient if the final byte wraps.
        assert_eq!(
            context.map_phys(PhysAddr::new(u64::MAX - 0x1000), 2, flags),
            Err(IommuError::MapFailed)
        );
        // A negative offset must not disguise overflow in the physical range.
        context.direct_dma_offset = -0x1000;
        assert_eq!(
            context.map_phys(PhysAddr::new(u64::MAX), 2, flags),
            Err(IommuError::MapFailed)
        );
        assert_eq!(
            context.map_phys(PhysAddr::new(0xfff), 1, flags),
            Err(IommuError::MapFailed)
        );
        assert_eq!(
            context.map_phys(PhysAddr::new(0x1000), 0, flags),
            Err(IommuError::MapFailed)
        );
    }

    #[test_case]
    fn iova_allocator_preserves_the_last_byte_of_its_address_space() {
        let base = Iova::new(u64::MAX - 0xfff);
        let mut allocator = DmaIovaAllocator::new(base, 0x1000);
        assert_eq!(allocator.alloc(0x1000, 0x1000), Ok(base));
        assert_eq!(allocator.alloc(1, 1), Err(IommuError::OutOfIova));
        allocator.free(base, 0x800);
        allocator.free(base.checked_add(0x800).unwrap(), 0x800);
        assert_eq!(allocator.alloc(0x1000, 0x1000), Ok(base));
        let mut invalid = DmaIovaAllocator::new(base, 0x1001);
        assert_eq!(invalid.alloc(1, 1), Err(IommuError::OutOfIova));
    }

    #[test_case]
    fn test_dma_context_direct_map_phys() {
        let context = DmaContext::direct();
        assert_eq!(
            context
                .map_phys(PhysAddr::new(0x1000), 0x100, IommuMapFlags::READ)
                .unwrap(),
            DmaAddr::new(0x1000)
        );
    }

    #[test_case]
    fn test_dma_context_iommu_map_phys_uses_identity_iova() {
        let domain = Arc::new(TestDomain::new());
        let controller = Arc::new(TestController {
            domain: domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller,
                domain: domain.clone(),
                streams: Vec::new(),
            }),
            Vec::new(),
            identity_iova_config(),
        );

        let dma_addr = context
            .map_phys(
                PhysAddr::new(0x2000),
                0x200,
                IommuMapFlags::READ | IommuMapFlags::WRITE,
            )
            .unwrap();
        assert_eq!(dma_addr, DmaAddr::new(0x2000));
        assert_eq!(
            domain.last_map(),
            Some(RecordedMap {
                iova: Iova::new(0x2000),
                paddr: PhysAddr::new(0x2000),
                len: 0x200,
                flags: IommuMapFlags::READ | IommuMapFlags::WRITE,
            })
        );
    }

    #[test_case]
    fn test_dma_context_maps_additional_iommus() {
        let primary_domain = Arc::new(TestDomain::new());
        let primary_controller = Arc::new(TestController {
            domain: primary_domain.clone(),
        });
        let secondary_domain = Arc::new(TestDomain::new());
        let secondary_controller = Arc::new(TestController {
            domain: secondary_domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller: primary_controller,
                domain: primary_domain.clone(),
                streams: Vec::new(),
            }),
            alloc::vec![IommuAttachment {
                controller: secondary_controller,
                domain: secondary_domain.clone(),
                streams: Vec::new(),
            }],
            identity_iova_config(),
        );

        let flags = IommuMapFlags::READ | IommuMapFlags::WRITE;
        let dma_addr = context
            .map_phys(PhysAddr::new(0x3000), 0x400, flags)
            .unwrap();
        assert_eq!(dma_addr, DmaAddr::new(0x3000));
        let expected = Some(RecordedMap {
            iova: Iova::new(0x3000),
            paddr: PhysAddr::new(0x3000),
            len: 0x400,
            flags,
        });
        assert_eq!(primary_domain.last_map(), expected);
        assert_eq!(secondary_domain.last_map(), expected);
    }

    #[test_case]
    fn test_dma_context_mapping_granule_uses_largest_iommu_page_size() {
        let primary_domain = Arc::new(TestDomain::with_page_size(0x1000));
        let primary_controller = Arc::new(TestController {
            domain: primary_domain.clone(),
        });
        let secondary_domain = Arc::new(TestDomain::with_page_size(0x4000));
        let secondary_controller = Arc::new(TestController {
            domain: secondary_domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller: primary_controller,
                domain: primary_domain,
                streams: Vec::new(),
            }),
            alloc::vec![IommuAttachment {
                controller: secondary_controller,
                domain: secondary_domain,
                streams: Vec::new(),
            }],
            identity_iova_config(),
        );

        assert_eq!(context.mapping_granule(), 0x4000);
    }

    #[test_case]
    fn test_dma_context_unmap_passthrough_when_no_iommu() {
        let context = DmaContext::direct();
        assert_eq!(context.unmap(DmaAddr::new(0x1000), 0x100), Ok(()));
    }

    #[test_case]
    fn test_dma_mapping_drop_unmaps_iommu_mapping() {
        let domain = Arc::new(TestDomain::new());
        let controller = Arc::new(TestController {
            domain: domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller,
                domain: domain.clone(),
                streams: Vec::new(),
            }),
            Vec::new(),
            identity_iova_config(),
        );

        {
            let mapping = context
                .map_phys_owned(PhysAddr::new(0x4000), 0x1000, IommuMapFlags::READ)
                .unwrap();
            assert_eq!(mapping.dma_addr(), DmaAddr::new(0x4000));
            assert_eq!(mapping.len(), 0x1000);
        }

        assert_eq!(domain.last_unmap(), Some((Iova::new(0x4000), 0x1000)));
        assert_eq!(domain.unmap_count(), 1);
    }

    #[test_case]
    fn test_dma_mapping_explicit_unmap_runs_once() {
        let domain = Arc::new(TestDomain::new());
        let controller = Arc::new(TestController {
            domain: domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller,
                domain: domain.clone(),
                streams: Vec::new(),
            }),
            Vec::new(),
            identity_iova_config(),
        );

        let mapping = context
            .map_phys_owned(PhysAddr::new(0x5000), 0x1000, IommuMapFlags::WRITE)
            .unwrap();
        mapping.unmap().unwrap();

        assert_eq!(domain.last_unmap(), Some((Iova::new(0x5000), 0x1000)));
        assert_eq!(domain.unmap_count(), 1);
    }

    #[test_case]
    fn test_dma_context_maps_scatter_segments_to_contiguous_iova() {
        let domain = Arc::new(TestDomain::new());
        let controller = Arc::new(TestController {
            domain: domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller,
                domain: domain.clone(),
                streams: Vec::new(),
            }),
            Vec::new(),
            allocated_iova_config(),
        );
        let flags = IommuMapFlags::READ | IommuMapFlags::WRITE;

        {
            let mapping = context
                .map_phys_segments_owned(
                    &[
                        (PhysAddr::new(0x10_0000), 0x2000),
                        (PhysAddr::new(0x30_0000), 0x1000),
                    ],
                    flags,
                )
                .unwrap();
            assert_eq!(mapping.dma_addr(), DmaAddr::new(0x4000_0000));
            assert_eq!(mapping.len(), 0x3000);
            assert_eq!(
                domain.maps(),
                alloc::vec![
                    RecordedMap {
                        iova: Iova::new(0x4000_0000),
                        paddr: PhysAddr::new(0x10_0000),
                        len: 0x2000,
                        flags,
                    },
                    RecordedMap {
                        iova: Iova::new(0x4000_2000),
                        paddr: PhysAddr::new(0x30_0000),
                        len: 0x1000,
                        flags,
                    },
                ]
            );
        }

        assert_eq!(domain.last_unmap(), Some((Iova::new(0x4000_0000), 0x3000)));
        assert_eq!(domain.unmap_count(), 1);
    }

    #[test_case]
    fn test_dma_context_rejects_scatter_without_iova_allocator() {
        let context = DmaContext::direct();
        assert_eq!(
            context
                .map_phys_segments_owned(
                    &[
                        (PhysAddr::new(0x10_0000), 0x1000),
                        (PhysAddr::new(0x30_0000), 0x1000)
                    ],
                    IommuMapFlags::READ,
                )
                .err(),
            Some(IommuError::NotSupported)
        );
    }

    #[test_case]
    fn test_dma_context_fixed_iova_mapping_uses_requested_address() {
        let domain = Arc::new(TestDomain::new());
        let controller = Arc::new(TestController {
            domain: domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller,
                domain: domain.clone(),
                streams: Vec::new(),
            }),
            Vec::new(),
            identity_iova_config(),
        );
        let flags = IommuMapFlags::READ | IommuMapFlags::EXECUTE;

        {
            let mapping = context
                .map_phys_at_owned(DmaAddr::new(0), PhysAddr::new(0x8f60_0000), 0x500000, flags)
                .unwrap();
            assert_eq!(mapping.dma_addr(), DmaAddr::new(0));
            assert_eq!(mapping.len(), 0x500000);
            assert_eq!(
                domain.last_map(),
                Some(RecordedMap {
                    iova: Iova::new(0),
                    paddr: PhysAddr::new(0x8f60_0000),
                    len: 0x500000,
                    flags,
                })
            );
        }

        assert_eq!(domain.last_unmap(), Some((Iova::new(0), 0x500000)));
        assert_eq!(domain.unmap_count(), 1);
    }

    #[test_case]
    fn test_dma_context_fixed_iova_rejects_allocator_context() {
        let domain = Arc::new(TestDomain::new());
        let controller = Arc::new(TestController {
            domain: domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller,
                domain: domain.clone(),
                streams: Vec::new(),
            }),
            Vec::new(),
            allocated_iova_config(),
        );

        assert_eq!(
            context
                .map_phys_at_owned(
                    DmaAddr::new(0),
                    PhysAddr::new(0x8f60_0000),
                    0x1000,
                    IommuMapFlags::READ
                )
                .err(),
            Some(IommuError::Busy)
        );
        assert_eq!(domain.last_map(), None);
    }

    #[test_case]
    fn test_dma_context_fixed_iova_rejects_direct_address_mismatch() {
        let context = DmaContext::direct();
        assert_eq!(
            context
                .map_phys_at_owned(
                    DmaAddr::new(0),
                    PhysAddr::new(0x8f60_0000),
                    0x1000,
                    IommuMapFlags::READ
                )
                .err(),
            Some(IommuError::NotSupported)
        );
    }

    #[test_case]
    fn test_dma_context_allocates_configured_iova_space() {
        let domain = Arc::new(TestDomain::with_page_size(0x4000));
        let controller = Arc::new(TestController {
            domain: domain.clone(),
        });
        let context = DmaContext::from_iommu_attachments(
            Some(IommuAttachment {
                controller,
                domain: domain.clone(),
                streams: Vec::new(),
            }),
            Vec::new(),
            allocated_iova_config(),
        );

        let dma_addr = context
            .map_phys(PhysAddr::new(0x8_0000_0000), 0x2000, IommuMapFlags::READ)
            .unwrap();
        assert_eq!(dma_addr, DmaAddr::new(0x4000_0000));
        assert_eq!(
            domain.last_map(),
            Some(RecordedMap {
                iova: Iova::new(0x4000_0000),
                paddr: PhysAddr::new(0x8_0000_0000),
                len: 0x4000,
                flags: IommuMapFlags::READ,
            })
        );

        context.unmap(dma_addr, 0x2000).unwrap();
        assert_eq!(domain.last_unmap(), Some((Iova::new(0x4000_0000), 0x4000)));

        let dma_addr = context
            .map_phys(PhysAddr::new(0x8_0004_0000), 0x1000, IommuMapFlags::WRITE)
            .unwrap();
        assert_eq!(dma_addr, DmaAddr::new(0x4000_0000));
    }
}
