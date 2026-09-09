//! Allocation-free RAM discovery for FDT boot adapters.
//!
//! Physical addresses and lengths are decoded at their firmware widths. Only
//! complete RAM pages enter the allocator; reservations exclude every page they
//! touch. `no-map` reservations also exclude direct-map aliases.

use crate::device::fdt::decode_address;
use crate::environment::PAGE_SIZE;
use crate::vm::direct_map::DirectMapRegions;
use crate::vm::vmem::{MemoryAttribute, PhysicalMemoryArea};

pub(crate) struct FdtMemory {
    pub direct_map: DirectMapRegions,
    pub usable: DirectMapRegions,
    pub initramfs: Option<PhysicalMemoryArea>,
}

impl FdtMemory {
    pub fn parse(
        blob: &[u8],
        kernel: PhysicalMemoryArea,
        dtb: PhysicalMemoryArea,
    ) -> Result<Self, &'static str> {
        let fdt = fdt::Fdt::new(blob).map_err(|_| "invalid boot FDT")?;
        let mut ram = DirectMapRegions::new();
        let mut reserved = DirectMapRegions::new();
        let mut no_map = DirectMapRegions::new();
        for node in fdt.all_nodes().filter(enabled) {
            if node.property("device_type").and_then(|p| p.as_str()) != Some("memory")
                && node.name != "memory"
                && !node.name.starts_with("memory@")
            {
                continue;
            }
            for reg in node.raw_reg().ok_or("RAM node has no reg property")? {
                if let Some(area) = reg_area(reg)? {
                    if let Some(pages) = complete_pages(area)? {
                        ram.insert(pages, MemoryAttribute::Normal)?;
                    }
                }
            }
        }
        if ram.is_empty() {
            return Err("FDT describes no RAM pages");
        }

        // fdt::memory_reservations exposes native pointers/usize lengths. Read
        // the firmware's fixed u64 pairs directly so RV32 cannot truncate them.
        for reservation in ReservationEntries::new(blob)? {
            reserved.insert(reservation?, MemoryAttribute::Normal)?;
        }
        if let Some(parent) = fdt.find_node("/reserved-memory") {
            // The standard binding requires an empty ranges property: these
            // reg values are already physical, with no bus translation.
            if !parent
                .property("ranges")
                .is_some_and(|p| p.value.is_empty())
            {
                return Err("reserved-memory requires empty ranges");
            }
            for node in parent.children().filter(enabled) {
                let regs = node
                    .raw_reg()
                    .ok_or("dynamic reserved-memory is unsupported at boot")?;
                for reg in regs {
                    if let Some(area) = reg_area(reg)? {
                        reserved.insert(area, MemoryAttribute::Normal)?;
                        if node.property("no-map").is_some() {
                            no_map.insert(area, MemoryAttribute::Normal)?;
                        }
                    }
                }
            }
        }
        let initramfs = initramfs_area(&fdt)?;
        let direct_map = subtract(&ram, &no_map)?;
        for area in [Some(kernel), Some(dtb), initramfs].into_iter().flatten() {
            if !direct_map.contains_area_with_attribute(area, MemoryAttribute::Normal) {
                return Err("boot object lies outside accessible RAM");
            }
            reserved.insert(area, MemoryAttribute::Normal)?;
        }
        let usable = subtract(&ram, &reserved)?;
        if usable.is_empty() {
            return Err("no unreserved RAM pages");
        }
        Ok(Self {
            direct_map,
            usable,
            initramfs,
        })
    }

    pub fn primary_usable(&self) -> PhysicalMemoryArea {
        (0..self.usable.len())
            .map(|i| self.usable.get(i).expect("usable region index").area())
            .max_by_key(|area| area.end - area.start)
            .expect("nonempty usable RAM")
    }
}

fn enabled(node: &fdt::node::FdtNode<'_, '_>) -> bool {
    node.property("status")
        .is_none_or(|p| matches!(p.as_str(), Some("okay" | "ok")))
}

fn reg_area(reg: fdt::node::RawReg<'_>) -> Result<Option<PhysicalMemoryArea>, &'static str> {
    let start = decode_address(reg.address).ok_or("invalid physical address cells")?;
    let size = decode_address(reg.size).ok_or("invalid physical size cells")?;
    area_from_size(start, size)
}

fn area_from_size(start: u64, size: u64) -> Result<Option<PhysicalMemoryArea>, &'static str> {
    if size == 0 {
        return Ok(None);
    }
    Ok(Some(PhysicalMemoryArea::new(
        start,
        start
            .checked_add(size - 1)
            .ok_or("physical region overflows")?,
    )))
}

fn complete_pages(area: PhysicalMemoryArea) -> Result<Option<PhysicalMemoryArea>, &'static str> {
    let mask = PAGE_SIZE as u64 - 1;
    let start = area
        .start
        .checked_add(mask)
        .ok_or("RAM page alignment overflows")?
        & !mask;
    let end = area.end.checked_add(1).ok_or("RAM end overflows")? & !mask;
    Ok((start < end).then(|| PhysicalMemoryArea::new(start, end - 1)))
}

/// Subtract sorted, page-aligned reservations without losing fragmented RAM.
fn subtract(
    ram: &DirectMapRegions,
    reserved: &DirectMapRegions,
) -> Result<DirectMapRegions, &'static str> {
    let mut result = DirectMapRegions::new();
    for i in 0..ram.len() {
        let area = ram.get(i).expect("RAM index").area();
        let mut cursor = area.start;
        for j in 0..reserved.len() {
            let reservation = reserved.get(j).expect("reservation index").area();
            if reservation.end < cursor {
                continue;
            }
            if reservation.start > area.end {
                break;
            }
            if cursor < reservation.start {
                result.insert(
                    PhysicalMemoryArea::new(cursor, reservation.start - 1),
                    MemoryAttribute::Normal,
                )?;
            }
            cursor = reservation
                .end
                .checked_add(1)
                .ok_or("reservation end overflows")?;
            if cursor > area.end {
                break;
            }
        }
        if cursor <= area.end {
            result.insert(
                PhysicalMemoryArea::new(cursor, area.end),
                MemoryAttribute::Normal,
            )?;
        }
    }
    Ok(result)
}

fn initramfs_area(fdt: &fdt::Fdt<'_>) -> Result<Option<PhysicalMemoryArea>, &'static str> {
    let Some(chosen) = fdt.find_node("/chosen") else {
        return Ok(None);
    };
    let start = chosen
        .property("linux,initrd-start")
        .or_else(|| chosen.property("initrd-start"));
    let end = chosen
        .property("linux,initrd-end")
        .or_else(|| chosen.property("initrd-end"));
    match (start, end) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) => {
            let start = decode_address(start.value).ok_or("invalid initrd start")?;
            let end = decode_address(end.value).ok_or("invalid initrd end")?;
            if end <= start {
                return Err("empty or reversed initrd range");
            }
            Ok(Some(PhysicalMemoryArea::new(start, end - 1)))
        }
        _ => Err("incomplete initrd range"),
    }
}

struct ReservationEntries<'a> {
    remaining: &'a [u8],
    done: bool,
}

impl<'a> ReservationEntries<'a> {
    fn new(blob: &'a [u8]) -> Result<Self, &'static str> {
        let total = u32::from_be_bytes(
            blob.get(4..8)
                .ok_or("short FDT header")?
                .try_into()
                .unwrap(),
        ) as usize;
        let offset = u32::from_be_bytes(
            blob.get(16..20)
                .ok_or("short FDT header")?
                .try_into()
                .unwrap(),
        ) as usize;
        if offset < 40 || offset & 7 != 0 {
            return Err("invalid FDT reservation offset");
        }
        let remaining = blob
            .get(..total)
            .and_then(|blob| blob.get(offset..))
            .ok_or("FDT reservations outside blob")?;
        Ok(Self {
            remaining,
            done: false,
        })
    }
}

impl Iterator for ReservationEntries<'_> {
    type Item = Result<PhysicalMemoryArea, &'static str>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            let Some(pair) = self.remaining.get(..16) else {
                self.done = true;
                return Some(Err("unterminated FDT reservations"));
            };
            self.remaining = &self.remaining[16..];
            let start = u64::from_be_bytes(pair[..8].try_into().unwrap());
            let size = u64::from_be_bytes(pair[8..].try_into().unwrap());
            if start == 0 && size == 0 {
                self.done = true;
                return None;
            }
            match area_from_size(start, size) {
                Ok(Some(area)) => return Some(Ok(area)),
                Ok(None) => continue,
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn reservation_subtraction_preserves_fragments_and_wide_addresses() {
        let mut ram = DirectMapRegions::new();
        let mut reserved = DirectMapRegions::new();
        let base = 0x1_0000_0000;
        ram.insert(
            PhysicalMemoryArea::new(base, base + 0xffff),
            MemoryAttribute::Normal,
        )
        .unwrap();
        reserved
            .insert(
                PhysicalMemoryArea::new(base + 0x2001, base + 0x30ff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        reserved
            .insert(
                PhysicalMemoryArea::new(base + 0xa000, base + 0xbfff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        let usable = subtract(&ram, &reserved).unwrap();
        assert_eq!(usable.len(), 3);
        assert_eq!(
            usable.get(0).unwrap().area(),
            PhysicalMemoryArea::new(base, base + 0x1fff)
        );
        assert_eq!(
            usable.get(1).unwrap().area(),
            PhysicalMemoryArea::new(base + 0x4000, base + 0x9fff)
        );
        assert_eq!(
            usable.get(2).unwrap().area(),
            PhysicalMemoryArea::new(base + 0xc000, base + 0xffff)
        );
    }

    #[test_case]
    fn reservation_table_decodes_u64_and_requires_a_terminator() {
        let mut blob = [0u8; 72];
        blob[4..8].copy_from_slice(&72u32.to_be_bytes());
        blob[16..20].copy_from_slice(&40u32.to_be_bytes());
        blob[40..48].copy_from_slice(&0x1_2345_6000u64.to_be_bytes());
        blob[48..56].copy_from_slice(&0x2_0000_0000u64.to_be_bytes());
        let mut entries = ReservationEntries::new(&blob).unwrap();
        assert_eq!(
            entries.next().unwrap().unwrap(),
            PhysicalMemoryArea::new(0x1_2345_6000, 0x3_2345_5fff)
        );
        assert!(entries.next().is_none());
        blob[4..8].copy_from_slice(&56u32.to_be_bytes());
        let mut entries = ReservationEntries::new(&blob).unwrap();
        assert!(entries.next().unwrap().is_ok());
        assert!(entries.next().unwrap().is_err());
    }
}
