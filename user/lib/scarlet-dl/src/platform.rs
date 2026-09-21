use crate::process::PAGE_SIZE;
#[cfg(target_os = "scarlet")]
use scarlet_loader_core::Platform;
use scarlet_loader_core::{Error, File, Permissions};
#[cfg(target_os = "scarlet")]
use scarlet_sys::{Syscall, syscall2, syscall3, syscall6};
#[cfg(target_os = "scarlet")]
use std::path::Path;

struct Mapping {
    start: usize,
    size: usize,
    allocation: Option<(usize, usize)>,
    pages: Vec<Permissions>,
}

/// All pointer access is constrained to allocations owned by this platform or
/// the main executable ranges explicitly adopted from the kernel handoff.
pub(crate) struct NativePlatform {
    mappings: Vec<Mapping>,
    main_file: Option<(String, File)>,
}

impl NativePlatform {
    /// # Safety
    /// Every range names persistent mapped pages of the main executable, and
    /// no code from that executable runs until relocation is complete.
    pub unsafe fn new(
        main_segments: &[(usize, usize, Permissions)],
        path: String,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            main_file: Some((
                path.clone(),
                File {
                    identity: path,
                    bytes,
                },
            )),
            mappings: main_segments
                .iter()
                .map(|&(start, size, permissions)| Mapping {
                    start,
                    size,
                    allocation: None,
                    pages: vec![permissions; size / PAGE_SIZE],
                })
                .collect(),
        }
    }

    fn checked_range(&self, address: usize, size: usize) -> Result<(), Error> {
        let end = address.checked_add(size).ok_or(Error::Overflow)?;
        let mut cursor = address;
        while cursor < end {
            let next = self
                .mappings
                .iter()
                .filter(|m| cursor >= m.start && cursor < m.start.saturating_add(m.size))
                .map(|m| m.start + m.size)
                .max()
                .ok_or_else(|| Error::Platform("access outside a loader mapping".into()))?;
            cursor = next.min(end);
        }
        Ok(())
    }

    fn check_access(&self, address: usize, size: usize, write: bool) -> Result<(), Error> {
        self.checked_range(address, size)?;
        let end = address.checked_add(size).ok_or(Error::Overflow)?;
        let mut page = address & !(PAGE_SIZE - 1);
        while page < end {
            let allowed = self
                .mappings
                .iter()
                .filter(|m| page >= m.start && page < m.start + m.size)
                .any(|m| {
                    let permissions = m.pages[(page - m.start) / PAGE_SIZE];
                    if write {
                        permissions.write
                    } else {
                        permissions.read
                    }
                });
            if !allowed {
                return Err(Error::Platform(
                    "image access violates page permissions".into(),
                ));
            }
            page = page.checked_add(PAGE_SIZE).ok_or(Error::Overflow)?;
        }
        Ok(())
    }
}

#[cfg(target_os = "scarlet")]
impl Platform for NativePlatform {
    fn read_file(&mut self, requester: Option<&str>, name: &str) -> Result<File, Error> {
        if requester.is_none()
            && self
                .main_file
                .as_ref()
                .is_some_and(|(path, _)| path == name)
        {
            return Ok(self.main_file.take().unwrap().1);
        }
        let mut candidates = Vec::new();
        if name.contains('/') {
            candidates.push(std::path::PathBuf::from(name));
        } else {
            if let Some(parent) = requester
                .filter(|p| Path::new(p).has_root())
                .and_then(|p| Path::new(p).parent())
            {
                candidates.push(parent.join(name));
            }
            candidates.push(Path::new("/system/lib").join(name));
            candidates.push(Path::new("/lib").join(name));
        }
        let mut last_error = None;
        for candidate in candidates {
            let resolved = crate::paths::resolve(&candidate)
                .and_then(|absolute| std::fs::read(&absolute).map(|bytes| (absolute, bytes)));
            match resolved {
                Ok((absolute, bytes)) => {
                    let identity = absolute
                        .to_str()
                        .ok_or(Error::Unsupported("non-UTF-8 library pathname"))?
                        .to_owned();
                    return Ok(File { identity, bytes });
                }
                Err(e) => last_error = Some(format!("{}: {e}", candidate.display())),
            }
        }
        Err(Error::Platform(format!(
            "cannot load {name}: {}",
            last_error.unwrap_or_default()
        )))
    }

    fn map(&mut self, size: usize, align: usize) -> Result<usize, Error> {
        let align = align.max(PAGE_SIZE);
        if size == 0 || size % PAGE_SIZE != 0 || !align.is_power_of_two() {
            return Err(Error::Platform(
                "invalid image mapping size or alignment".into(),
            ));
        }
        let reserve = size.checked_add(align - PAGE_SIZE).ok_or(Error::Overflow)?;
        // SAFETY: Request a fresh private anonymous RW, non-executable region.
        let raw = unsafe { syscall6(Syscall::MemoryMap, 0, 0, reserve, 3, 0x22, 0) };
        if raw == usize::MAX || raw == 0 {
            return Err(Error::Platform("anonymous image mapping failed".into()));
        }
        let Some(aligned) = raw.checked_add(align - 1).map(|a| a & !(align - 1)) else {
            // SAFETY: This fresh mapping has not escaped.
            unsafe {
                syscall2(Syscall::MemoryUnmap, raw, reserve);
            }
            return Err(Error::Overflow);
        };
        self.mappings.push(Mapping {
            start: aligned,
            size,
            allocation: Some((raw, reserve)),
            pages: vec![
                Permissions {
                    read: true,
                    write: true,
                    execute: false
                };
                size / PAGE_SIZE
            ],
        });
        Ok(aligned)
    }

    fn write(&mut self, address: usize, bytes: &[u8]) -> Result<(), Error> {
        self.check_access(address, bytes.len(), true)?;
        // SAFETY: Loader-core only writes its unpublished, writable image
        // ranges during relocation. The platform checked allocation bounds.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len());
        }
        Ok(())
    }

    fn read(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error> {
        self.check_access(address, bytes.len(), false)?;
        // SAFETY: These are initialized image bytes within a loader mapping.
        unsafe {
            std::ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), bytes.len());
        }
        Ok(())
    }

    fn protect(
        &mut self,
        address: usize,
        size: usize,
        permissions: Permissions,
    ) -> Result<(), Error> {
        self.checked_range(address, size)?;
        if address % PAGE_SIZE != 0
            || size == 0
            || size % PAGE_SIZE != 0
            || (permissions.write && permissions.execute)
        {
            return Err(Error::Platform(
                "invalid protection range or writable executable page".into(),
            ));
        }
        let flags = usize::from(permissions.read)
            | (usize::from(permissions.write) << 1)
            | (usize::from(permissions.execute) << 2);
        // SAFETY: The context owns these mapped pages. Native MemoryProtect
        // also performs the executable-page instruction-cache synchronization.
        let result = unsafe { syscall3(Syscall::MemoryProtect, address, size, flags) };
        if result == usize::MAX {
            Err(Error::Platform(format!(
                "MemoryProtect failed at {address:#x} ({size:#x}, flags {flags})"
            )))
        } else {
            let end = address + size;
            for mapping in &mut self.mappings {
                let begin = address.max(mapping.start);
                let limit = end.min(mapping.start + mapping.size);
                if begin < limit {
                    for page in &mut mapping.pages
                        [(begin - mapping.start) / PAGE_SIZE..(limit - mapping.start) / PAGE_SIZE]
                    {
                        *page = permissions;
                    }
                }
            }
            Ok(())
        }
    }

    fn unmap(&mut self, address: usize, size: usize) {
        if let Some(index) = self
            .mappings
            .iter()
            .position(|m| m.start == address && m.size == size && m.allocation.is_some())
        {
            let mapping = self.mappings.swap_remove(index);
            let (raw, reserve) = mapping.allocation.unwrap();
            // SAFETY: Core drops only unpublished failed loads or its entire
            // context. The resident runtime never drops a published context.
            unsafe {
                syscall2(Syscall::MemoryUnmap, raw, reserve);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(start: usize, pages: usize, write: bool) -> Mapping {
        Mapping {
            start,
            size: pages * PAGE_SIZE,
            allocation: None,
            pages: vec![
                Permissions {
                    read: true,
                    write,
                    execute: false
                };
                pages
            ],
        }
    }

    #[test]
    fn range_can_span_adjacent_or_overlapping_main_segments() {
        let platform = NativePlatform {
            main_file: None,
            mappings: vec![
                mapping(0x1000, 2, true),
                mapping(0x2000, 2, true),
                mapping(0x4000, 1, true),
            ],
        };
        assert!(platform.checked_range(0x1000, 0x4000).is_ok());
        assert!(platform.checked_range(0x1000, 0x4001).is_err());
        assert!(platform.checked_range(usize::MAX - 1, 4).is_err());
    }

    #[test]
    fn gaps_and_read_only_pages_are_rejected() {
        let platform = NativePlatform {
            main_file: None,
            mappings: vec![
                mapping(0x1000, 1, true),
                mapping(0x2000, 1, false),
                mapping(0x4000, 1, true),
            ],
        };
        assert!(platform.check_access(0x1ff8, 16, false).is_ok());
        assert!(platform.check_access(0x1ff8, 16, true).is_err());
        assert!(platform.check_access(0x1000, 16, true).is_ok());
        assert!(platform.checked_range(0x1000, 0x4000).is_err());
    }
}
