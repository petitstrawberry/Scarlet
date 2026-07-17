//! Sparse physical-memory regions mapped by Scarlet's higher-half direct map.

use crate::environment::PAGE_SIZE;
use crate::vm::vmem::{MemoryArea, MemoryAttribute};

/// Maximum number of sparse physical regions in Scarlet's direct map.
pub const MAX_DIRECT_MAP_REGIONS: usize = 64;

/// One physical region and the memory attribute used for its direct-map alias.
#[derive(Clone, Copy, Debug)]
pub struct DirectMapRegion {
    area: MemoryArea,
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
    pub fn new(area: MemoryArea, memory_attribute: MemoryAttribute) -> Result<Self, &'static str> {
        if area.start > area.end {
            return Err("direct-map region has an invalid physical range");
        }

        let start = area.start & !(PAGE_SIZE - 1);
        let end_exclusive = area
            .end
            .checked_add(1)
            .ok_or("direct-map region end overflows while aligning")?;
        let aligned_end_exclusive = end_exclusive
            .checked_add(PAGE_SIZE - 1)
            .ok_or("direct-map region end overflows while aligning")?
            & !(PAGE_SIZE - 1);
        let end = aligned_end_exclusive
            .checked_sub(1)
            .ok_or("direct-map region has an empty aligned range")?;

        Ok(Self {
            area: MemoryArea::new(start, end),
            memory_attribute,
        })
    }

    /// Returns the inclusive, page-aligned physical range for this region.
    ///
    /// # Returns
    ///
    /// The physical memory area represented by this region.
    pub const fn area(&self) -> MemoryArea {
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
/// Regions retain [`MemoryArea`]'s inclusive-end semantics. Overlapping or
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
        area: MemoryArea,
        memory_attribute: MemoryAttribute,
    ) -> Result<(), &'static str> {
        let region = DirectMapRegion::new(area, memory_attribute)?;
        self.insert_region(region)
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
    pub fn contains(&self, paddr: usize) -> bool {
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
        area: MemoryArea,
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
        area: MemoryArea,
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
    pub fn bounding_area(&self) -> Option<MemoryArea> {
        let first = self.get(0)?;
        let last = self.get(self.len.checked_sub(1)?)?;
        Some(MemoryArea::new(first.area.start, last.area.end))
    }

    fn region_containing(&self, paddr: usize) -> Option<DirectMapRegion> {
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
                    let merged = MemoryArea::new(
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
}

impl Default for DirectMapRegions {
    fn default() -> Self {
        Self::new()
    }
}

fn areas_overlap(left: MemoryArea, right: MemoryArea) -> bool {
    left.start <= right.end && right.start <= left.end
}

fn areas_touch_or_overlap(left: MemoryArea, right: MemoryArea) -> bool {
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
            .insert(MemoryArea::new(0x1003, 0x1fff), MemoryAttribute::Normal)
            .unwrap();
        regions
            .insert(MemoryArea::new(0x2000, 0x2fff), MemoryAttribute::Normal)
            .unwrap();

        assert_eq!(regions.len(), 1);
        assert_eq!(
            regions.get(0).unwrap().area(),
            MemoryArea::new(0x1000, 0x2fff)
        );
    }

    #[test_case]
    fn direct_map_regions_keep_holes_outside_membership() {
        let mut regions = DirectMapRegions::new();
        regions
            .insert(MemoryArea::new(0x1000, 0x1fff), MemoryAttribute::Normal)
            .unwrap();
        regions
            .insert(MemoryArea::new(0x3000, 0x3fff), MemoryAttribute::Normal)
            .unwrap();

        assert!(regions.contains(0x1000));
        assert!(!regions.contains(0x2000));
        assert!(regions.contains(0x3fff));
    }

    #[test_case]
    fn direct_map_regions_reject_conflicting_attributes_and_aliases() {
        let mut regions = DirectMapRegions::new();
        regions
            .insert(MemoryArea::new(0x1000, 0x1fff), MemoryAttribute::Normal)
            .unwrap();

        assert!(
            regions
                .insert(MemoryArea::new(0x1800, 0x2fff), MemoryAttribute::Device)
                .is_err()
        );
        assert!(
            regions
                .validate_alias(MemoryArea::new(0x1000, 0x1fff), MemoryAttribute::Normal)
                .is_ok()
        );
        assert!(
            regions
                .validate_alias(MemoryArea::new(0x1000, 0x1fff), MemoryAttribute::Device)
                .is_err()
        );
    }
}
