//! Publish only complete native extents. A boundary page keeps the exact
//! resident guest ranges, including holes and distinct backing identities.
use super::{Extent, Image, Perm, Protection, Worker};
use crate::{mem::page::ContiguousPages, vm::vmem::MappingOrigin};
use alloc::vec::Vec;
use std::{io, os::fd::AsFd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Part {
    offset: usize,
    len: usize,
    pa: u64,
    origin: MappingOrigin,
    vm_start: usize,
    perms: usize,
}

pub(super) fn insert(
    desired: &mut Vec<(usize, Extent)>,
    page: usize,
    mut address: usize,
    mut extent: Extent,
) {
    while extent.len != 0 {
        let offset = address % page;
        if Perm::Read.contained_in(extent.perms)
            && offset == 0
            && extent.pa.is_multiple_of(page as u64)
            && extent.len >= page
        {
            let len = extent.len / page * page;
            desired.push((
                address,
                Extent {
                    len,
                    ..extent.clone()
                },
            ));
            address += len;
            extent.pa += len as u64;
            extent.len -= len;
            continue;
        }
        let len = extent.len.min(page - offset);
        let base = address - offset;
        if desired.last().is_none_or(|(previous, _)| *previous != base) {
            desired.push((
                base,
                Extent {
                    len: page,
                    ..extent.clone()
                },
            ));
        }
        let (_, group) = desired.last_mut().unwrap();
        group.perms |= extent.perms;
        group.parts.push(Part {
            offset,
            len,
            pa: extent.pa,
            origin: extent.origin,
            vm_start: extent.vm_start,
            perms: extent.perms,
        });
        address += len;
        extent.pa += len as u64;
        extent.len -= len;
    }
}

pub(super) fn publish(
    worker: &mut Worker,
    image: &Image,
    address: usize,
    extent: &Extent,
) -> io::Result<()> {
    let page = extent.len;
    let parts = &extent.parts;
    let mut end = 0;
    let mut continuous = parts[0].pa.is_multiple_of(page as u64);
    let mut writable = true;
    let mut readable = true;
    let mut execute: Vec<core::ops::Range<usize>> = Vec::new();
    for part in parts {
        if part.offset < end || part.offset + part.len > page {
            return Err(io::Error::other("Overlapping guest fragments"));
        }
        continuous &= part.offset == end && part.pa == parts[0].pa + part.offset as u64;
        readable &= Perm::Read.contained_in(part.perms);
        writable &= Perm::Write.contained_in(part.perms);
        end = part.offset + part.len;
        if Perm::Execute.contained_in(part.perms) {
            if Perm::Write.contained_in(part.perms) {
                return Err(io::Error::other("Writable executable guest fragment"));
            }
            // The snapshot is published atomically. Contiguous X fragments
            // may share native control flow even with different data backing.
            if let Some(previous) = execute.last_mut()
                && previous.end == part.offset
            {
                previous.end = end;
            } else {
                execute.push(part.offset..end);
            }
        }
    }
    // A complete RX host page can execute at its guest VA even when its
    // backing comes from multiple 4 KiB allocations. Prove coverage and
    // instruction provenance separately for each fragment before publishing.
    let mut instructions = Vec::new();
    let mut covered = 0;
    for part in parts {
        if part.offset != covered
            || !Perm::Read.contained_in(part.perms)
            || !Perm::Execute.contained_in(part.perms)
        {
            break;
        }
        let first = instructions.len();
        let start = address + part.offset;
        image.append_instruction_ranges(
            part.origin,
            part.vm_start,
            start..start + part.len,
            address,
            &mut instructions,
        );
        if instructions.len() == first {
            break;
        }
        covered += part.len;
    }
    let native_text = covered == page;
    if native_text {
        instructions.sort_by_key(|range| range.start);
    }
    if continuous
        && end == page
        && readable
        && let Ok((file, offset)) = super::super::memory::worker_backing(parts[0].pa, page)
        && offset.is_multiple_of(page)
    {
        // One live, coherent data view. Stores that require finer protection
        // fault and are checked against the original guest VMAs.
        let protection = if native_text {
            Protection::ReadExecute(&instructions)
        } else if !execute.is_empty() {
            Protection::ReadExecuteOnFetch(&execute)
        } else if writable {
            Protection::ReadWrite
        } else {
            Protection::ReadOnly
        };
        return worker.map(address, page, file.as_fd(), offset, protection);
    }
    if execute.is_empty() {
        return worker.guard(address, page);
    }
    // The worker consumes this snapshot before acknowledging publication.
    // Incomplete or mixed-permission fragments keep their inaccessible data view.
    let scratch = ContiguousPages::new_aligned(page / crate::environment::PAGE_SIZE, page)
        .ok_or(io::ErrorKind::OutOfMemory)?;
    for part in parts.iter().filter(|p| Perm::Execute.contained_in(p.perms)) {
        let source = crate::hosted::alias_for(part.pa)
            .ok_or_else(|| io::Error::other("Missing executable fragment backing"))?;
        let last = part.len - 1;
        if crate::hosted::alias_for(part.pa + last as u64) != source.checked_add(last) {
            return Err(io::Error::other(
                "Discontiguous executable fragment backing",
            ));
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                source as *const u8,
                scratch.as_ptr().cast::<u8>().add(part.offset),
                part.len,
            );
        }
    }
    let (file, offset) = super::super::memory::worker_backing(scratch.as_paddr(), page)?;
    worker.map(
        address,
        page,
        file.as_fd(),
        offset,
        if native_text {
            Protection::ReadExecute(&instructions)
        } else {
            Protection::ExecuteOnFetch(&execute)
        },
    )
}
