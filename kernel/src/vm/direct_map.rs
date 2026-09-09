//! Sparse physical-memory regions mapped by Scarlet's higher-half direct map.

use crate::environment::PAGE_SIZE;
use crate::mem::address::{PhysAddr, VirtAddr};
use crate::vm::vmem::{MemoryAttribute, PhysicalMemoryArea};

/// Maximum number of sparse physical regions in Scarlet's direct map.
pub const MAX_DIRECT_MAP_REGIONS: usize = 64;

/// One physical region and the memory attribute used for its direct-map alias.
#[derive(Clone, Copy, Debug)]
pub struct DirectMapRegion {
    area: PhysicalMemoryArea,
    memory_attribute: MemoryAttribute,
}

impl DirectMapRegion {
    /// Creates a page-aligned direct-map region.
    ///
    /// # Arguments
    ///
    /// * `area` - Inclusive physical range to include in the direct map.
    /// * `memory_attribute` - Attribute applied to every mapped page in `area`.
    ///
    /// # Returns
    ///
    /// A page-aligned region, or an error if the input range is invalid or
    /// cannot be aligned without overflowing.
    pub fn new(
        area: PhysicalMemoryArea,
        memory_attribute: MemoryAttribute,
    ) -> Result<Self, &'static str> {
        if area.start > area.end {
            return Err("direct-map region has an invalid physical range");
        }

        let start = area.start & !(PAGE_SIZE as u64 - 1);
        let end_exclusive = area
            .end
            .checked_add(1)
            .ok_or("direct-map region end overflows while aligning")?;
        let aligned_end_exclusive = end_exclusive
            .checked_add(PAGE_SIZE as u64 - 1)
            .ok_or("direct-map region end overflows while aligning")?
            & !(PAGE_SIZE as u64 - 1);
        let end = aligned_end_exclusive
            .checked_sub(1)
            .ok_or("direct-map region has an empty aligned range")?;

        Ok(Self {
            area: PhysicalMemoryArea::new(start, end),
            memory_attribute,
        })
    }

    /// Returns the inclusive, page-aligned physical range for this region.
    ///
    /// # Returns
    ///
    /// The physical memory area represented by this region.
    pub const fn area(&self) -> PhysicalMemoryArea {
        self.area
    }

    /// Returns the memory attribute used by this region's direct-map alias.
    ///
    /// # Returns
    ///
    /// The attribute used when mapping this region.
    pub const fn memory_attribute(&self) -> MemoryAttribute {
        self.memory_attribute
    }
}

/// Fixed-capacity, sorted set of attribute-aware sparse direct-map regions.
///
/// Regions retain [`PhysicalMemoryArea`]'s inclusive-end semantics. Overlapping or
/// adjacent equal-attribute regions merge, while overlapping different-
/// attribute regions are rejected so one physical page cannot gain conflicting
/// aliases.
#[derive(Clone, Copy, Debug)]
pub struct DirectMapRegions {
    regions: [Option<DirectMapRegion>; MAX_DIRECT_MAP_REGIONS],
    len: usize,
}

impl DirectMapRegions {
    /// Creates an empty sparse direct-map region set.
    ///
    /// # Returns
    ///
    /// An empty fixed-capacity region set.
    pub const fn new() -> Self {
        Self {
            regions: [None; MAX_DIRECT_MAP_REGIONS],
            len: 0,
        }
    }

    /// Returns the number of regions currently stored.
    ///
    /// # Returns
    ///
    /// The number of non-empty direct-map regions.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether this region set is empty.
    ///
    /// # Returns
    ///
    /// `true` when no regions have been inserted.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns a region by sorted index.
    ///
    /// # Arguments
    ///
    /// * `index` - Zero-based region index.
    ///
    /// # Returns
    ///
    /// The requested region, or `None` if `index` is outside the set.
    pub fn get(&self, index: usize) -> Option<DirectMapRegion> {
        if index >= self.len {
            return None;
        }
        self.regions[index]
    }

    /// Inserts a physical range with the requested direct-map attribute.
    ///
    /// The range is page-aligned with checked arithmetic before insertion.
    /// Adjacent or overlapping regions merge only when their attributes match.
    ///
    /// # Arguments
    ///
    /// * `area` - Inclusive physical range to insert.
    /// * `memory_attribute` - Attribute for the direct-map alias.
    ///
    /// # Returns
    ///
    /// `Ok(())` on insertion, or an error for an invalid range, capacity
    /// overflow, or conflicting overlapping attributes.
    pub fn insert(
        &mut self,
        area: PhysicalMemoryArea,
        memory_attribute: MemoryAttribute,
    ) -> Result<(), &'static str> {
        let region = DirectMapRegion::new(area, memory_attribute)?;
        self.insert_region(region)
    }

    /// Retags a fully covered, page-aligned direct-map range.
    ///
    /// The operation splits regions at the requested boundaries and merges
    /// adjacent equal-attribute regions. The requested range must be fully
    /// covered without holes and must currently have one uniform attribute.
    /// Validation and capacity checks complete before this set is changed.
    ///
    /// # Arguments
    ///
    /// * `area` - Inclusive, page-aligned physical range to retag.
    /// * `memory_attribute` - Attribute for the retagged direct-map range.
    ///
    /// # Returns
    ///
    /// The original uniform attribute on success, or an error when the range
    /// is invalid, partially covered, mixed, or cannot fit after splitting.
    pub fn retag(
        &mut self,
        area: PhysicalMemoryArea,
        memory_attribute: MemoryAttribute,
    ) -> Result<MemoryAttribute, &'static str> {
        if area.start > area.end
            || !area.start.is_multiple_of(PAGE_SIZE as u64)
            || area.end % PAGE_SIZE as u64 != PAGE_SIZE as u64 - 1
        {
            return Err("direct-map retag range must be page-aligned and non-empty");
        }

        let mut first = None;
        let mut last = None;
        let mut next_covered = area.start;
        let mut original_attribute = None;

        for (index, region) in self.regions[..self.len].iter().flatten().enumerate() {
            if region.area.end < area.start {
                continue;
            }
            if region.area.start > area.end {
                break;
            }

            let overlap_start = region.area.start.max(area.start);
            let overlap_end = region.area.end.min(area.end);
            if overlap_start != next_covered {
                return Err("direct-map retag range is not fully covered");
            }

            match original_attribute {
                Some(original) if original != region.memory_attribute => {
                    return Err("direct-map retag range has mixed memory attributes");
                }
                None => original_attribute = Some(region.memory_attribute),
                _ => {}
            }

            first.get_or_insert(index);
            last = Some(index);
            if overlap_end == area.end {
                next_covered = area.end;
                break;
            }
            next_covered = overlap_end
                .checked_add(1)
                .ok_or("direct-map retag range coverage overflows")?;
        }

        if first.is_none() || next_covered != area.end {
            return Err("direct-map retag range is not fully covered");
        }

        let original_attribute = original_attribute.expect("covered range has an attribute");
        if original_attribute == memory_attribute {
            return Ok(original_attribute);
        }

        let first = first.expect("covered range has a first region");
        let last = last.expect("covered range has a last region");
        let mut replacement = [None; MAX_DIRECT_MAP_REGIONS];
        let mut replacement_len = 0;

        for (index, region) in self.regions[..self.len]
            .iter()
            .flatten()
            .copied()
            .enumerate()
        {
            if index < first || index > last {
                Self::append_region(&mut replacement, &mut replacement_len, region)?;
                continue;
            }

            if index == first {
                if region.area.start < area.start {
                    Self::append_region(
                        &mut replacement,
                        &mut replacement_len,
                        DirectMapRegion {
                            area: PhysicalMemoryArea::new(region.area.start, area.start - 1),
                            memory_attribute: region.memory_attribute,
                        },
                    )?;
                }
                Self::append_region(
                    &mut replacement,
                    &mut replacement_len,
                    DirectMapRegion {
                        area,
                        memory_attribute,
                    },
                )?;
            }

            if index == last && region.area.end > area.end {
                Self::append_region(
                    &mut replacement,
                    &mut replacement_len,
                    DirectMapRegion {
                        area: PhysicalMemoryArea::new(area.end + 1, region.area.end),
                        memory_attribute: region.memory_attribute,
                    },
                )?;
            }
        }

        self.regions = replacement;
        self.len = replacement_len;
        Ok(original_attribute)
    }

    /// Returns whether a physical address belongs to a sparse direct-map region.
    ///
    /// # Arguments
    ///
    /// * `paddr` - Physical address to inspect.
    ///
    /// # Returns
    ///
    /// `true` if one recorded region contains `paddr`.
    pub fn contains(&self, paddr: u64) -> bool {
        self.region_containing(paddr).is_some()
    }

    /// Returns whether one region fully contains an area with the given attribute.
    ///
    /// # Arguments
    ///
    /// * `area` - Inclusive physical range to inspect.
    /// * `memory_attribute` - Required direct-map memory attribute.
    ///
    /// # Returns
    ///
    /// `true` if one recorded region fully contains `area` with the requested
    /// attribute.
    pub fn contains_area_with_attribute(
        &self,
        area: PhysicalMemoryArea,
        memory_attribute: MemoryAttribute,
    ) -> bool {
        self.regions[..self.len].iter().flatten().any(|region| {
            region.memory_attribute == memory_attribute
                && region.area.start <= area.start
                && area.end <= region.area.end
        })
    }

    /// Validates an additional physical alias against the direct-map attributes.
    ///
    /// # Arguments
    ///
    /// * `area` - Inclusive physical range for the additional alias.
    /// * `memory_attribute` - Attribute requested for the additional alias.
    ///
    /// # Returns
    ///
    /// `Ok(())` if no direct-map region overlaps, or every overlapping region
    /// uses the same attribute. Returns an error for a conflicting overlap.
    pub fn validate_alias(
        &self,
        area: PhysicalMemoryArea,
        memory_attribute: MemoryAttribute,
    ) -> Result<(), &'static str> {
        if area.start > area.end {
            return Err("direct-map alias has an invalid physical range");
        }

        for region in self.regions[..self.len].iter().flatten() {
            if areas_overlap(area, region.area) && region.memory_attribute != memory_attribute {
                return Err("physical mapping conflicts with direct-map memory attribute");
            }
        }

        Ok(())
    }

    /// Returns the inclusive bounds that cover all sparse regions.
    ///
    /// This compatibility view can include holes and must not be used for
    /// membership validation.
    ///
    /// # Returns
    ///
    /// `Some(bounds)` for a non-empty set, or `None` when no regions exist.
    pub fn bounding_area(&self) -> Option<PhysicalMemoryArea> {
        let first = self.get(0)?;
        let last = self.get(self.len.checked_sub(1)?)?;
        Some(PhysicalMemoryArea::new(first.area.start, last.area.end))
    }

    fn region_containing(&self, paddr: u64) -> Option<DirectMapRegion> {
        self.regions[..self.len]
            .iter()
            .flatten()
            .copied()
            .find(|region| region.area.start <= paddr && paddr <= region.area.end)
    }

    fn insert_region(&mut self, mut candidate: DirectMapRegion) -> Result<(), &'static str> {
        loop {
            let mut changed = false;
            for existing in self.regions[..self.len].iter().flatten() {
                if areas_overlap(existing.area, candidate.area)
                    && existing.memory_attribute != candidate.memory_attribute
                {
                    return Err("direct-map regions overlap with different memory attributes");
                }

                if existing.memory_attribute == candidate.memory_attribute
                    && areas_touch_or_overlap(existing.area, candidate.area)
                {
                    let merged = PhysicalMemoryArea::new(
                        existing.area.start.min(candidate.area.start),
                        existing.area.end.max(candidate.area.end),
                    );
                    if merged.start != candidate.area.start || merged.end != candidate.area.end {
                        candidate.area = merged;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let mut replacement = [None; MAX_DIRECT_MAP_REGIONS];
        let mut replacement_len = 0;
        let mut inserted = false;

        for existing in self.regions[..self.len].iter().flatten().copied() {
            if existing.memory_attribute == candidate.memory_attribute
                && areas_touch_or_overlap(existing.area, candidate.area)
            {
                continue;
            }

            if !inserted && candidate.area.start < existing.area.start {
                if replacement_len == MAX_DIRECT_MAP_REGIONS {
                    return Err("direct-map region capacity exceeded");
                }
                replacement[replacement_len] = Some(candidate);
                replacement_len += 1;
                inserted = true;
            }

            if replacement_len == MAX_DIRECT_MAP_REGIONS {
                return Err("direct-map region capacity exceeded");
            }
            replacement[replacement_len] = Some(existing);
            replacement_len += 1;
        }

        if !inserted {
            if replacement_len == MAX_DIRECT_MAP_REGIONS {
                return Err("direct-map region capacity exceeded");
            }
            replacement[replacement_len] = Some(candidate);
            replacement_len += 1;
        }

        self.regions = replacement;
        self.len = replacement_len;
        Ok(())
    }

    fn append_region(
        regions: &mut [Option<DirectMapRegion>; MAX_DIRECT_MAP_REGIONS],
        len: &mut usize,
        candidate: DirectMapRegion,
    ) -> Result<(), &'static str> {
        if let Some(previous) = regions[(*len).saturating_sub(1)]
            && previous.memory_attribute == candidate.memory_attribute
            && areas_touch_or_overlap(previous.area, candidate.area)
        {
            regions[*len - 1] = Some(DirectMapRegion {
                area: PhysicalMemoryArea::new(
                    previous.area.start.min(candidate.area.start),
                    previous.area.end.max(candidate.area.end),
                ),
                memory_attribute: previous.memory_attribute,
            });
            return Ok(());
        }

        if *len == MAX_DIRECT_MAP_REGIONS {
            return Err("direct-map region capacity exceeded");
        }
        regions[*len] = Some(candidate);
        *len += 1;
        Ok(())
    }
}

impl Default for DirectMapRegions {
    fn default() -> Self {
        Self::new()
    }
}

fn areas_overlap(left: PhysicalMemoryArea, right: PhysicalMemoryArea) -> bool {
    left.start <= right.end && right.start <= left.end
}

fn areas_touch_or_overlap(left: PhysicalMemoryArea, right: PhysicalMemoryArea) -> bool {
    areas_overlap(left, right)
        || left.end.checked_add(1) == Some(right.start)
        || right.end.checked_add(1) == Some(left.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn direct_map_regions_align_and_merge_matching_regions() {
        let mut regions = DirectMapRegions::new();
        regions
            .insert(
                PhysicalMemoryArea::new(0x1003, 0x1fff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        regions
            .insert(
                PhysicalMemoryArea::new(0x2000, 0x2fff),
                MemoryAttribute::Normal,
            )
            .unwrap();

        assert_eq!(regions.len(), 1);
        assert_eq!(
            regions.get(0).unwrap().area(),
            PhysicalMemoryArea::new(0x1000, 0x2fff)
        );
    }

    #[test_case]
    fn direct_map_regions_keep_holes_outside_membership() {
        let mut regions = DirectMapRegions::new();
        regions
            .insert(
                PhysicalMemoryArea::new(0x1000, 0x1fff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        regions
            .insert(
                PhysicalMemoryArea::new(0x3000, 0x3fff),
                MemoryAttribute::Normal,
            )
            .unwrap();

        assert!(regions.contains(0x1000));
        assert!(!regions.contains(0x2000));
        assert!(regions.contains(0x3fff));
    }

    #[test_case]
    fn direct_map_regions_reject_conflicting_attributes_and_aliases() {
        let mut regions = DirectMapRegions::new();
        regions
            .insert(
                PhysicalMemoryArea::new(0x1000, 0x1fff),
                MemoryAttribute::Normal,
            )
            .unwrap();

        assert!(
            regions
                .insert(
                    PhysicalMemoryArea::new(0x1800, 0x2fff),
                    MemoryAttribute::Device
                )
                .is_err()
        );
        assert!(
            regions
                .validate_alias(
                    PhysicalMemoryArea::new(0x1000, 0x1fff),
                    MemoryAttribute::Normal
                )
                .is_ok()
        );
        assert!(
            regions
                .validate_alias(
                    PhysicalMemoryArea::new(0x1000, 0x1fff),
                    MemoryAttribute::Device
                )
                .is_err()
        );
    }

    #[test_case]
    fn direct_map_regions_retag_splits_a_region_and_returns_original_attribute() {
        let mut regions = DirectMapRegions::new();
        regions
            .insert(
                PhysicalMemoryArea::new(0x1000, 0x3fff),
                MemoryAttribute::Normal,
            )
            .unwrap();

        assert_eq!(
            regions
                .retag(
                    PhysicalMemoryArea::new(0x2000, 0x2fff),
                    MemoryAttribute::DeviceBurstable,
                )
                .unwrap(),
            MemoryAttribute::Normal
        );
        assert_eq!(regions.len(), 3);
        assert_eq!(
            regions.get(0).unwrap().area(),
            PhysicalMemoryArea::new(0x1000, 0x1fff)
        );
        assert_eq!(
            regions.get(1).unwrap().memory_attribute(),
            MemoryAttribute::DeviceBurstable
        );
        assert_eq!(
            regions.get(2).unwrap().area(),
            PhysicalMemoryArea::new(0x3000, 0x3fff)
        );
    }

    #[test_case]
    fn direct_map_regions_retag_merges_matching_neighbors() {
        let mut regions = DirectMapRegions::new();
        regions
            .insert(
                PhysicalMemoryArea::new(0x1000, 0x1fff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        regions
            .insert(
                PhysicalMemoryArea::new(0x2000, 0x2fff),
                MemoryAttribute::DeviceBurstable,
            )
            .unwrap();
        regions
            .insert(
                PhysicalMemoryArea::new(0x3000, 0x3fff),
                MemoryAttribute::Normal,
            )
            .unwrap();

        regions
            .retag(
                PhysicalMemoryArea::new(0x2000, 0x2fff),
                MemoryAttribute::Normal,
            )
            .unwrap();

        assert_eq!(regions.len(), 1);
        assert_eq!(
            regions.get(0).unwrap().area(),
            PhysicalMemoryArea::new(0x1000, 0x3fff)
        );
        assert_eq!(
            regions.get(0).unwrap().memory_attribute(),
            MemoryAttribute::Normal
        );
    }

    #[test_case]
    fn direct_map_regions_retag_rejects_gaps_and_mixed_source_attributes() {
        let mut regions = DirectMapRegions::new();
        regions
            .insert(
                PhysicalMemoryArea::new(0x1000, 0x1fff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        regions
            .insert(
                PhysicalMemoryArea::new(0x3000, 0x3fff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        assert!(
            regions
                .retag(
                    PhysicalMemoryArea::new(0x1000, 0x3fff),
                    MemoryAttribute::Device
                )
                .is_err()
        );
        assert_eq!(regions.len(), 2);

        let mut mixed = DirectMapRegions::new();
        mixed
            .insert(
                PhysicalMemoryArea::new(0x1000, 0x1fff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        mixed
            .insert(
                PhysicalMemoryArea::new(0x2000, 0x2fff),
                MemoryAttribute::DeviceBurstable,
            )
            .unwrap();
        assert!(
            mixed
                .retag(
                    PhysicalMemoryArea::new(0x1000, 0x2fff),
                    MemoryAttribute::Device
                )
                .is_err()
        );
        assert_eq!(
            mixed.get(0).unwrap().memory_attribute(),
            MemoryAttribute::Normal
        );
        assert_eq!(
            mixed.get(1).unwrap().memory_attribute(),
            MemoryAttribute::DeviceBurstable
        );
        assert!(
            mixed
                .retag(
                    PhysicalMemoryArea::new(0x1001, 0x1fff),
                    MemoryAttribute::Device
                )
                .is_err()
        );
    }

    #[test_case]
    fn direct_map_regions_retag_capacity_failure_does_not_mutate() {
        let mut regions = DirectMapRegions::new();
        for index in 0..MAX_DIRECT_MAP_REGIONS {
            let start = 0x1000 + (index * PAGE_SIZE * 4) as u64;
            regions
                .insert(
                    PhysicalMemoryArea::new(start, start + PAGE_SIZE as u64 * 3 - 1),
                    MemoryAttribute::Normal,
                )
                .unwrap();
        }
        let original_first = regions.get(0).unwrap();

        assert!(
            regions
                .retag(
                    PhysicalMemoryArea::new(
                        original_first.area().start + PAGE_SIZE as u64,
                        original_first.area().start + PAGE_SIZE as u64 * 2 - 1,
                    ),
                    MemoryAttribute::Device,
                )
                .is_err()
        );
        assert_eq!(regions.len(), MAX_DIRECT_MAP_REGIONS);
        assert_eq!(regions.get(0).unwrap().area(), original_first.area());
        assert_eq!(
            regions.get(0).unwrap().memory_attribute(),
            MemoryAttribute::Normal
        );
    }
}

/// A linear mapping with independent physical and virtual origins.
///
/// This describes address arithmetic only; the sparse region set determines
/// which pages actually exist. No wrapping arithmetic is used at either edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectMapWindow {
    physical_base: PhysAddr,
    virtual_base: VirtAddr,
    size: usize,
}

impl DirectMapWindow {
    pub fn new(
        physical_base: PhysAddr,
        virtual_base: VirtAddr,
        size: usize,
    ) -> Result<Self, &'static str> {
        let last = size.checked_sub(1).ok_or("direct-map window is empty")?;
        physical_base
            .checked_add(last as u64)
            .ok_or("direct-map physical window overflows")?;
        virtual_base
            .checked_add(last)
            .ok_or("direct-map virtual window overflows")?;
        Ok(Self {
            physical_base,
            virtual_base,
            size,
        })
    }

    /// Convert a Limine-style additive offset and bounds at the boot-protocol boundary.
    pub fn from_offset(offset: usize, bounds: PhysicalMemoryArea) -> Result<Self, &'static str> {
        let virtual_base = (offset as u64)
            .checked_add(bounds.start)
            .and_then(|address| usize::try_from(address).ok())
            .ok_or("boot direct-map virtual origin overflows")?;
        let size = bounds
            .byte_len()
            .and_then(|size| usize::try_from(size).ok())
            .ok_or("boot direct-map bounds exceed pointer width")?;
        Self::new(
            PhysAddr::new(bounds.start),
            VirtAddr::new(virtual_base),
            size,
        )
    }

    pub const fn physical_base(self) -> PhysAddr {
        self.physical_base
    }

    pub const fn virtual_base(self) -> VirtAddr {
        self.virtual_base
    }

    pub const fn size(self) -> usize {
        self.size
    }

    pub fn phys_to_virt(self, paddr: PhysAddr) -> Option<VirtAddr> {
        let offset = usize::try_from(paddr.checked_offset_from(self.physical_base)?).ok()?;
        if offset >= self.size {
            return None;
        }
        self.virtual_base.checked_add(offset)
    }

    pub fn virt_to_phys(self, vaddr: VirtAddr) -> Option<PhysAddr> {
        let offset = vaddr.checked_offset_from(self.virtual_base)?;
        if offset >= self.size {
            return None;
        }
        self.physical_base.checked_add(offset as u64)
    }

    pub fn physical_area(self) -> PhysicalMemoryArea {
        PhysicalMemoryArea::new(
            self.physical_base.as_u64(),
            self.physical_base.as_u64() + (self.size as u64 - 1),
        )
    }

    /// Choose the physical origin for the architecture's bounded kernel VA window.
    pub fn for_kernel(
        regions: &DirectMapRegions,
        initramfs: Option<PhysicalMemoryArea>,
    ) -> Result<Self, &'static str> {
        let mut bounds = regions
            .bounding_area()
            .ok_or("kernel direct-map regions are empty")?;
        if let Some(area) = initramfs {
            bounds.start = bounds.start.min(area.start);
            bounds.end = bounds.end.max(area.end);
        }
        #[cfg(target_pointer_width = "64")]
        let physical_base = 0;
        #[cfg(target_pointer_width = "32")]
        let physical_base = bounds.start & !(PAGE_SIZE as u64 - 1);
        let window = Self::new(
            PhysAddr::new(physical_base),
            VirtAddr::new(crate::environment::SCARLET_HHDM_BASE),
            crate::environment::KERNEL_DIRECT_MAP_SIZE,
        )?;
        if window.phys_to_virt(PhysAddr::new(bounds.start)).is_none()
            || window.phys_to_virt(PhysAddr::new(bounds.end)).is_none()
        {
            return Err("physical memory span exceeds the kernel direct-map VA capacity");
        }
        Ok(window)
    }
}

#[cfg(test)]
#[test_case]
fn direct_map_window_preserves_high_physical_addresses() {
    let window = DirectMapWindow::new(
        PhysAddr::new(0x1_8000_0000),
        VirtAddr::new(0xc000_0000),
        0x1000,
    )
    .unwrap();
    assert_eq!(
        window.phys_to_virt(PhysAddr::new(0x1_8000_0123)),
        Some(VirtAddr::new(0xc000_0123))
    );
    assert_eq!(
        window.virt_to_phys(VirtAddr::new(0xc000_0fff)),
        Some(PhysAddr::new(0x1_8000_0fff))
    );
    assert_eq!(window.phys_to_virt(PhysAddr::new(0x8000_0123)), None);
    assert_eq!(window.phys_to_virt(PhysAddr::new(0x1_8000_1000)), None);
    assert_eq!(window.virt_to_phys(VirtAddr::new(0xbfff_ffff)), None);
    assert!(DirectMapWindow::new(PhysAddr::ZERO, VirtAddr::new(usize::MAX), 2).is_err());
    assert!(DirectMapWindow::new(PhysAddr::new(u64::MAX), VirtAddr::ZERO, 2).is_err());
    assert!(DirectMapWindow::new(PhysAddr::ZERO, VirtAddr::ZERO, 0).is_err());
}
