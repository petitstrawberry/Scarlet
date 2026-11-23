//! Host-page anonymous mappings owned by a single in-process guest VM.

use std::{collections::BTreeMap, io, ops::Range, ptr};

const READ: usize = 1;
const WRITE: usize = 2;

#[derive(Clone, Copy)]
struct Mapping {
    end: usize,
    protection: usize,
}

pub struct AnonymousMappings {
    page_size: usize,
    mappings: BTreeMap<usize, Mapping>,
}

impl AnonymousMappings {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            page_size: crate::ram::host_page_size()?,
            mappings: BTreeMap::new(),
        })
    }

    /// The caller serializes guest CPU admission and kernel accesses with this
    /// operation for the lifetime of the returned mapping.
    ///
    /// # Safety
    ///
    /// No running guest or borrowed kernel alias may use a range being replaced.
    pub unsafe fn map(
        &mut self,
        fixed: Option<usize>,
        length: usize,
        protection: usize,
    ) -> io::Result<usize> {
        let native = host_protection(protection)?;
        let length = self.rounded_length(length)?;
        let address = match fixed {
            Some(address) => {
                self.owned_range(address, length)?;
                ptr::with_exposed_provenance_mut(address)
            }
            None => ptr::null_mut(),
        };
        let flags = libc::MAP_PRIVATE | libc::MAP_ANON;
        // SAFETY: MAP_FIXED is used only inside this VM's exclusive ownership.
        let result = unsafe {
            libc::mmap(
                address,
                length,
                native,
                flags | if fixed.is_some() { libc::MAP_FIXED } else { 0 },
                -1,
                0,
            )
        };
        if result == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let base = result.expose_provenance();
        let end = base + length;
        if fixed.is_some() {
            self.remove_entries(base..end);
        }
        self.mappings.insert(base, Mapping { end, protection });
        Ok(base)
    }

    /// # Safety
    ///
    /// No running guest or borrowed kernel alias may use the affected range.
    pub unsafe fn protect(
        &mut self,
        address: usize,
        length: usize,
        protection: usize,
    ) -> io::Result<()> {
        let native = host_protection(protection)?;
        if length == 0 || !length.is_multiple_of(self.page_size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Protection changes require whole host pages",
            ));
        }
        let range = self.owned_range(address, length)?;
        // SAFETY: the entire host-page range belongs to this VM.
        if unsafe { libc::mprotect(ptr::with_exposed_provenance_mut(address), length, native) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        self.split(range.start);
        self.split(range.end);
        for (_, map) in self.mappings.range_mut(range) {
            map.protection = protection;
        }
        Ok(())
    }

    /// # Safety
    ///
    /// No running guest or borrowed kernel alias may use the affected range.
    pub unsafe fn unmap(&mut self, address: usize, length: usize) -> io::Result<()> {
        let length = self.rounded_length(length)?;
        let range = self.owned_range(address, length)?;
        // SAFETY: the entire host-page range belongs to this VM.
        if unsafe { libc::munmap(ptr::with_exposed_provenance_mut(address), length) } != 0 {
            return Err(io::Error::last_os_error());
        }
        self.remove_entries(range);
        Ok(())
    }

    pub fn bounds(&self, address: usize, write: bool) -> Option<Range<usize>> {
        let (&start, map) = self.mappings.range(..=address).next_back()?;
        let needed = if write { WRITE } else { READ };
        (address < map.end && map.protection & needed != 0).then_some(start..map.end)
    }

    fn rounded_length(&self, length: usize) -> io::Result<usize> {
        length
            .checked_next_multiple_of(self.page_size)
            .filter(|&length| length != 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid mapping length"))
    }

    fn owned_range(&self, address: usize, length: usize) -> io::Result<Range<usize>> {
        let unowned = || io::Error::new(io::ErrorKind::InvalidInput, "Mapping is outside this VM");
        let end = address
            .checked_add(length)
            .filter(|_| address.is_multiple_of(self.page_size))
            .ok_or_else(unowned)?;
        let (&base, _) = self
            .mappings
            .range(..=address)
            .next_back()
            .ok_or_else(unowned)?;
        let mut covered = address;
        for (&start, map) in self.mappings.range(base..end) {
            if start > covered || map.end <= covered {
                break;
            }
            covered = map.end;
            if covered >= end {
                return Ok(address..end);
            }
        }
        Err(unowned())
    }

    fn split(&mut self, address: usize) {
        if let Some((_, previous)) = self.mappings.range_mut(..address).next_back()
            && address < previous.end
        {
            let right = *previous;
            previous.end = address;
            self.mappings.insert(address, right);
        }
    }

    fn remove_entries(&mut self, range: Range<usize>) {
        self.split(range.start);
        self.split(range.end);
        let keys: Vec<_> = self
            .mappings
            .range(range)
            .map(|(&start, _)| start)
            .collect();
        for start in keys {
            self.mappings.remove(&start);
        }
    }
}

impl Drop for AnonymousMappings {
    fn drop(&mut self) {
        for (&start, map) in &self.mappings {
            // SAFETY: no caller can use a range after dropping its VM.
            unsafe { libc::munmap(ptr::with_exposed_provenance_mut(start), map.end - start) };
        }
    }
}

fn host_protection(protection: usize) -> io::Result<libc::c_int> {
    if protection & !(READ | WRITE) != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Anonymous guest protection is limited to read and write",
        ));
    }
    let mut native = libc::PROT_NONE;
    if protection & READ != 0 {
        native |= libc::PROT_READ;
    }
    if protection & WRITE != 0 {
        native |= libc::PROT_WRITE;
    }
    Ok(native)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_replacement_and_protection_require_owned_host_pages() {
        let mut first = AnonymousMappings::new().unwrap();
        let mut second = AnonymousMappings::new().unwrap();
        let page = first.page_size;
        // SAFETY: no guest is running. The test uses host pointers only after
        // requesting the required permissions.
        unsafe {
            let reservation = first.map(None, 3 * page, 0).unwrap();
            let other = second.map(None, page, READ | WRITE).unwrap();
            assert!(first.bounds(reservation, false).is_none());
            assert!(first.map(Some(other), page, READ).is_err());
            assert!(first.protect(other, page, READ).is_err());
            assert!(first.unmap(other, page).is_err());
            assert!(first.protect(reservation + 1, page, READ).is_err());
            assert!(first.map(Some(reservation), page, 4).is_err());
            let middle = reservation + page;
            assert_eq!(first.map(Some(middle), 1, READ | WRITE).unwrap(), middle);
            let data = ptr::with_exposed_provenance_mut::<u8>(middle);
            assert_eq!(*data, 0);
            *data = 42;
            assert!(first.protect(middle, 1, READ).is_err());
            first.protect(middle, page, READ).unwrap();
            assert_eq!(*data, 42);
            assert_eq!(first.bounds(middle, false), Some(middle..middle + page));
            assert!(first.bounds(middle, true).is_none());
            assert!(first.protect(middle, page, 4).is_err());
            assert_eq!(*data, 42);
            assert_eq!(first.map(Some(middle), page, READ | WRITE).unwrap(), middle);
            assert_eq!(*data, 0);
            first.unmap(middle, 1).unwrap();
            assert!(first.bounds(middle, false).is_none());
            assert!(first.protect(reservation, 3 * page, READ).is_err());
            assert!(first.map(Some(reservation), 3 * page, READ).is_err());
            assert!(first.unmap(reservation, 3 * page).is_err());
            first.protect(reservation + 2 * page, page, READ).unwrap();
            assert!(first.bounds(reservation + 2 * page, false).is_some());
            assert!(second.bounds(other, true).is_some());
        }
    }
}
