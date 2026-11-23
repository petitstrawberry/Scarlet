//! Shared RAM backing for Scarlet's logical frame addresses.

#[cfg(target_os = "ios")]
mod local;
#[cfg(target_os = "ios")]
pub use local::LocalWindow;

use std::{
    fs::File,
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd},
    ptr::NonNull,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
};

pub fn host_page_size() -> io::Result<usize> {
    // SAFETY: sysconf has no pointer arguments.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size)
        .ok()
        .filter(|size| size.is_power_of_two())
        .ok_or_else(|| io::Error::other("Invalid host page size"))
}

pub struct RamArena {
    base: u64,
    len: usize,
    alias: NonNull<u8>,
    backing: File,
}

// The API provides raw aliases. Synchronization of accesses to mapped bytes
// belongs to the VM manager, which also keeps the backing alive for each map.
unsafe impl Send for RamArena {}
unsafe impl Sync for RamArena {}

pub struct FrameRange<'a> {
    pub backing: BorrowedFd<'a>,
    pub offset: usize,
    pub len: usize,
    pub alias: NonNull<u8>,
}

impl RamArena {
    /// Reserve a kernel alias backed by a shared object suitable for mapping
    /// into another process. Logical addresses remain independent of the fd.
    pub fn new(base: u64, len: usize) -> io::Result<Self> {
        let page_size = host_page_size()?;
        if base == 0
            || !base.is_multiple_of(page_size as u64)
            || len == 0
            || !len.is_multiple_of(page_size)
            || base.checked_add(len as u64).is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "RAM arena must be a nonempty aligned logical range",
            ));
        }
        let file_len = libc::off_t::try_from(len).map_err(|_| io::ErrorKind::InvalidInput)?;
        let backing = create_backing(file_len)?;
        // SAFETY: The file retains the complete initialized extent. The host
        // chooses a free aligned address for the kernel's read/write alias.
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                backing.as_raw_fd(),
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let Some(alias) = NonNull::new(mapping.cast()) else {
            // SAFETY: mmap reported a successful mapping at address zero.
            unsafe { libc::munmap(mapping, len) };
            return Err(io::Error::other("RAM alias mapped at the null address"));
        };
        Ok(Self {
            base,
            len,
            alias,
            backing,
        })
    }

    /// Resolve logical frame bytes without I/O or taking a lock.
    pub fn resolve(&self, address: u64, len: usize) -> Option<FrameRange<'_>> {
        let offset = usize::try_from(address.checked_sub(self.base)?).ok()?;
        if len == 0 || offset.checked_add(len)? > self.len {
            return None;
        }
        Some(FrameRange {
            backing: self.backing.as_fd(),
            offset,
            len,
            // SAFETY: The checked nonempty range is within the live mapping.
            alias: unsafe { self.alias.add(offset) },
        })
    }

    pub fn logical_address(&self, alias: usize) -> Option<u64> {
        let offset = alias.checked_sub(self.alias.as_ptr().addr())?;
        (offset < self.len).then(|| self.base + offset as u64)
    }
}

impl Drop for RamArena {
    fn drop(&mut self) {
        // SAFETY: The arena owns this complete mapping until its final drop.
        unsafe { libc::munmap(self.alias.as_ptr().cast(), self.len) };
    }
}

/// A stable host alias reserved for a shared object. Commit only the rounded
/// prefix that has been allocated. A committed prefix is never remapped.
pub struct RamReservation {
    limit: usize,
    alias: NonNull<u8>,
    page_size: usize,
    committed: AtomicUsize,
    growth: Mutex<()>,
    extents: RwLock<Vec<Extent>>,
}

struct Extent {
    start: usize,
    end: usize,
    backing: Arc<File>,
}

pub struct ReservedFrameRange {
    pub backing: Arc<File>,
    pub offset: usize,
    pub alias: NonNull<u8>,
}

// The reservation is owned and all access to bytes is through raw aliases.
unsafe impl Send for RamReservation {}
unsafe impl Sync for RamReservation {}

impl RamReservation {
    pub fn reserve(limit: usize) -> io::Result<Self> {
        let page_size = host_page_size()?;
        if limit == 0 || !limit.is_multiple_of(page_size) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let alias = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                limit,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if alias == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let Some(alias) = NonNull::new(alias.cast()) else {
            unsafe { libc::munmap(alias, limit) };
            return Err(io::Error::other("RAM reservation mapped at address zero"));
        };
        Ok(Self {
            limit,
            alias,
            page_size,
            committed: AtomicUsize::new(0),
            growth: Mutex::new(()),
            extents: RwLock::new(Vec::new()),
        })
    }

    pub fn commit(&self, size: usize) -> io::Result<()> {
        let size = size.checked_next_multiple_of(self.page_size)
            .filter(|&size| size <= self.limit)
            .ok_or(io::ErrorKind::InvalidInput)?;
        let _growth = self.growth.lock().unwrap();
        let old_size = self.committed.load(Ordering::Relaxed);
        if size <= old_size {
            return Ok(());
        }
        let len = libc::off_t::try_from(size - old_size).map_err(|_| io::ErrorKind::InvalidInput)?;
        let backing = Arc::new(create_backing(len)?);
        // SAFETY: The uncommitted tail belongs exclusively to this reservation.
        // No worker can observe it until the successfully mapped prefix is published.
        let address = unsafe { self.alias.add(old_size).as_ptr() }.cast();
        let mapped = unsafe {
            libc::mmap(
                address,
                size - old_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                backing.as_raw_fd(),
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        self.extents.write().unwrap().push(Extent { start: old_size, end: size, backing });
        self.committed.store(size, Ordering::Release);
        Ok(())
    }

    pub fn alias_base(&self) -> usize {
        self.alias.as_ptr().addr()
    }

    pub fn alias(&self, offset: usize, len: usize) -> Option<NonNull<u8>> {
        if len == 0 || offset.checked_add(len)? > self.committed.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: Commit published this extent before the acquire above.
        Some(unsafe { self.alias.add(offset) })
    }

    /// Length of the contiguous prefix available from one backing descriptor.
    pub fn extent_length(&self, offset: usize, limit: usize) -> Option<usize> {
        self.alias(offset, 1)?;
        let extents = self.extents.read().unwrap();
        let index = extents.partition_point(|extent| extent.end <= offset);
        Some(limit.min(extents.get(index)?.end - offset))
    }

    /// Resolve bytes covered by one host extent. A range spanning multiple
    /// extents must be split before sending a mapping to a worker.
    pub fn resolve(&self, offset: usize, len: usize) -> Option<ReservedFrameRange> {
        let alias = self.alias(offset, len)?;
        let extents = self.extents.read().unwrap();
        let extent = extents.get(extents.partition_point(|extent| extent.end <= offset))?;
        if offset.checked_add(len)? > extent.end {
            return None;
        }
        Some(ReservedFrameRange {
            backing: extent.backing.clone(),
            offset: offset - extent.start,
            alias,
        })
    }
}

impl Drop for RamReservation {
    fn drop(&mut self) {
        // SAFETY: The entire virtual extent, including committed prefixes,
        // remains owned by the reservation until this destructor.
        unsafe { libc::munmap(self.alias.as_ptr().cast(), self.limit) };
    }
}

#[cfg(target_os = "linux")]
fn create_backing(len: libc::off_t) -> io::Result<File> {
    let fd = unsafe { libc::memfd_create(c"scarlet-ram".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let backing = unsafe { File::from_raw_fd(fd) };
    backing.set_len(len as u64)?;
    Ok(backing)
}

#[cfg(target_vendor = "apple")]
fn create_backing(len: libc::off_t) -> io::Result<File> {
    use std::{ffi::CString, sync::atomic::AtomicU64};

    static NEXT_ARENA: AtomicU64 = AtomicU64::new(0);
    let backing = loop {
        let next = NEXT_ARENA
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| io::Error::other("RAM backing identifiers exhausted"))?;
        // Leave room for the PID and counter under Darwin's name limit.
        let name = CString::new(format!("/scar-{:x}-{next:x}", std::process::id())).unwrap();
        // SAFETY: name points to a NUL-terminated exclusive name. No other
        // process receives access through the namespace.
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                libc::c_uint::from(libc::S_IRUSR | libc::S_IWUSR),
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(error);
        }
        // SAFETY: The new descriptor is uniquely owned. Unlink immediately so
        // future users can access it only through the descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        if unsafe { libc::shm_unlink(name.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        break file;
    };
    backing.set_len(len as u64)?;
    Ok(backing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_mapping_and_kernel_alias_share_the_same_bytes() {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let arena = RamArena::new(page as u64, 2 * page).unwrap();
        let logical = (2 * page) as u64;
        let range = arena.resolve(logical, page).unwrap();
        assert_eq!(
            arena.logical_address(range.alias.as_ptr().addr()),
            Some(logical)
        );
        assert_eq!(range.offset, page);
        assert!(arena.resolve(logical + page as u64, 1).is_none());
        // SAFETY: The aligned, borrowed range comes from the live shared object.
        let other = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                page,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                range.backing.as_raw_fd(),
                range.offset as libc::off_t,
            )
        };
        assert_ne!(other, libc::MAP_FAILED);
        let other = other.cast::<u8>();
        let offset = 4096.min(page - 1);
        // SAFETY: These are valid aliases accessed sequentially by one thread.
        unsafe {
            range.alias.as_ptr().add(offset).write_volatile(1);
            assert_eq!(other.add(offset).read_volatile(), 1);
            other.add(offset).write_volatile(2);
            assert_eq!(range.alias.as_ptr().add(offset).read_volatile(), 2);
            libc::munmap(other.cast(), page);
        }
    }

    #[test]
    fn reservation_growth_preserves_prefix_address_and_contents() {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let ram = RamReservation::reserve(4 * page).unwrap();
        assert!(ram.resolve(0, 1).is_none());
        ram.commit(page).unwrap();
        let before = ram.resolve(0, 1).unwrap();
        // SAFETY: This test alone owns access to the reservation's bytes.
        unsafe { before.alias.as_ptr().write_volatile(0x45) };
        assert!(ram.resolve(page, 1).is_none());
        ram.commit(3 * page).unwrap();
        let after = ram.resolve(0, 1).unwrap();
        let tail = ram.resolve(page, 1).unwrap();
        assert_eq!(before.alias, after.alias);
        assert_eq!(before.backing.as_raw_fd(), after.backing.as_raw_fd());
        assert_eq!(unsafe { after.alias.as_ptr().read_volatile() }, 0x45);
        assert_eq!(unsafe { tail.alias.as_ptr().read_volatile() }, 0);
        assert_ne!(before.backing.as_raw_fd(), tail.backing.as_raw_fd());
        assert!(ram.alias(0, 3 * page).is_some());
        assert!(ram.commit(5 * page).is_err());
    }
}
