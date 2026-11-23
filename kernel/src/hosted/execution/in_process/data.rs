//! Publish the resident data pages of a VMM inside an owned host VA window.

use super::super::{Image, Perm, visit_resident_extents};
use crate::vm::{
    manager::VirtualMemoryManager,
    vmem::{MemoryArea, VirtualMemoryMap},
};
use alloc::{collections::BTreeMap, vec::Vec};
use core::ops::Range;
use scarlet_hosted_runtime::ram::LocalWindow;
use std::{io, os::fd::AsFd, sync::Mutex};

// Only virtual address space is reserved. The ordinary Scarlet owners supply
// physical backing on demand, including when a process forks.
const WINDOW_SIZE: usize = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
struct Page {
    physical: u64,
    writable: bool,
}

pub(super) struct DataWindow {
    range: Range<usize>,
    publication: Mutex<Publication>,
}

struct Publication {
    window: LocalWindow,
    installed: BTreeMap<usize, Page>,
    resident: Option<u64>,
}

impl DataWindow {
    pub(super) fn new() -> io::Result<Self> {
        let window = LocalWindow::reserve(WINDOW_SIZE)?;
        Ok(Self {
            range: window.range(),
            publication: Mutex::new(Publication {
                window,
                installed: BTreeMap::new(),
                resident: None,
            }),
        })
    }

    pub(super) fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub(super) fn contains(&self, address: usize) -> bool {
        self.range.contains(&address)
    }

    /// With the VMM locked and the carrier parked, choose host mappings from
    /// the VMM's resident extents. A fork sibling uses this same window and
    /// publishes its own physical pages at the original virtual addresses.
    pub(super) fn publish(&self, vm: &VirtualMemoryManager, generation: u64) -> io::Result<()> {
        let granule = crate::vm::execution_page_size();
        let desired = vm.memmaps_iter_with(|maps| {
            let mut desired = BTreeMap::new();
            for map in maps.filter(|m| Image::is_user_data(m.permissions)) {
                if !self.contains(map.vmarea.start) || !self.contains(map.vmarea.end) {
                    return Err(io::Error::other("VMA is outside its reserved data window"));
                }
                let mut invalid_extent = false;
                visit_resident_extents(map, |address, physical, size, permissions| {
                    if !Image::is_user_data(permissions) {
                        return;
                    }
                    if !address.is_multiple_of(granule)
                        || !physical.is_multiple_of(granule as u64)
                        || !size.is_multiple_of(granule)
                    {
                        invalid_extent = true;
                        return;
                    }
                    for page in (address..address + size).step_by(granule) {
                        desired.insert(
                            page,
                            Page {
                                physical: physical + (page - address) as u64,
                                writable: Perm::Write.contained_in(permissions),
                            },
                        );
                    }
                });
                if invalid_extent {
                    return Err(io::Error::other(
                        "VMA resident extent violates host page alignment",
                    ));
                }
            }
            Ok(desired)
        })?;
        let mut publication = self.publication.lock().unwrap();
        if let Err(error) = publication.apply(desired, granule) {
            // A failed fork-sibling change can have installed pages from both
            // address spaces. Revoke all before either sibling can retire RAM.
            if let Err(reset) = publication.revoke(0, usize::MAX, granule) {
                std::eprintln!("Cannot revoke mixed guest aliases: {reset}");
                std::process::abort();
            }
            publication.resident = None;
            return Err(error);
        }
        publication.resident = Some(generation);
        Ok(())
    }

    fn retire(&self, generation: u64, start: usize, end: usize) -> io::Result<()> {
        let mut publication = self.publication.lock().unwrap();
        if publication.resident != Some(generation) {
            return Ok(());
        }
        publication.revoke(start, end, crate::vm::execution_page_size())
    }
}

impl Publication {
    fn apply(&mut self, desired: BTreeMap<usize, Page>, granule: usize) -> io::Result<()> {
        let removed: Vec<_> = self
            .installed
            .keys()
            .copied()
            .filter(|address| !desired.contains_key(address))
            .collect();
        for address in removed {
            // The VM owns its mapping transaction and no guest is executing.
            unsafe { self.window.replace_page(address, None)? };
            self.installed.remove(&address);
        }
        for (address, page) in desired {
            if self.installed.get(&address) != Some(&page) {
                let (file, offset) =
                    super::super::super::memory::worker_backing(page.physical, granule)?;
                unsafe {
                    self.window
                        .replace_page(address, Some((file.as_fd(), offset, page.writable)))?
                };
                self.installed.insert(address, page);
            }
        }
        Ok(())
    }

    fn revoke(&mut self, start: usize, end: usize, granule: usize) -> io::Result<()> {
        let removed: Vec<_> = self
            .installed
            .range((start & !(granule - 1))..=end)
            .map(|(&address, _)| address)
            .collect();
        for address in removed {
            unsafe { self.window.replace_page(address, None)? };
            self.installed.remove(&address);
        }
        Ok(())
    }
}

impl Image {
    pub(super) fn is_user_data(permissions: usize) -> bool {
        Perm::User.contained_in(permissions)
            && Perm::Read.contained_in(permissions)
            && !Perm::Execute.contained_in(permissions)
    }

    pub(crate) fn retire_data_aliases(&self, start: usize, end: usize) {
        let local = self.local.lock().unwrap();
        if let Some(local) = &*local
            && let Err(error) = local.data.retire(local.generation, start, end)
        {
            std::eprintln!("Cannot retire a hosted data alias: {error}");
            std::process::abort();
        }
    }

    pub(crate) fn validate_data_change<'a>(
        &self,
        area: MemoryArea,
        maps: impl Iterator<Item = &'a VirtualMemoryMap>,
    ) -> Result<(), &'static str> {
        if !self.has_signed_mapping() {
            return Ok(());
        }
        let granule = crate::vm::execution_page_size();
        for map in maps.filter(|m| Self::is_user_data(m.permissions)) {
            let start = area.start.max(map.vmarea.start);
            let end = area.end.min(map.vmarea.end);
            if start <= end && (!start.is_multiple_of(granule) || end % granule != granule - 1) {
                return Err("Cannot split accessible data within a host protection page");
            }
        }
        Ok(())
    }

    pub(crate) fn validate_data_backing(&self, map: &VirtualMemoryMap) -> Result<(), &'static str> {
        if !self.has_signed_mapping() || !Self::is_user_data(map.permissions) {
            return Ok(());
        }
        let granule = crate::vm::execution_page_size();
        let mut valid = map.pmarea.start.is_multiple_of(granule as u64);
        visit_resident_extents(map, |address, physical, size, permissions| {
            if Self::is_user_data(permissions) {
                valid &= address.is_multiple_of(granule)
                    && physical.is_multiple_of(granule as u64)
                    && size.is_multiple_of(granule);
            }
        });
        if !valid {
            return Err("Physical backing extent is not host-page aligned");
        }
        Ok(())
    }

    pub(crate) fn validate_data_mapping(
        &self,
        area: MemoryArea,
        permissions: usize,
    ) -> Result<(), &'static str> {
        let local = self.local.lock().unwrap();
        let Some(local) = local.as_ref() else {
            return Ok(());
        };
        if !local.data.contains(area.start) || !local.data.contains(area.end) {
            return Err("Mapping is outside the VM's reserved data window");
        }
        if permissions & 7 != 0 {
            let granule = crate::vm::execution_page_size();
            if !Self::is_user_data(permissions)
                || !area.start.is_multiple_of(granule)
                || !area.size().is_multiple_of(granule)
            {
                return Err(
                    "Mapping permissions or alignment cannot be published in the data window",
                );
            }
        }
        Ok(())
    }
}
