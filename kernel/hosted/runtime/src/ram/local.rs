//! Non-executable aliases inside a reserved window of the current host process.

use std::{
    io,
    ops::Range,
    os::fd::{AsRawFd, BorrowedFd},
    ptr,
};

pub struct LocalWindow {
    range: Range<usize>,
    page: usize,
}

impl LocalWindow {
    pub fn reserve(size: usize) -> io::Result<Self> {
        let page = super::host_page_size()?;
        if size == 0 || !size.is_multiple_of(page) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_NONE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let base = base.expose_provenance();
        Ok(Self {
            range: base..base + size,
            page,
        })
    }

    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// Replace a page without returning its address to the host allocator.
    ///
    /// # Safety
    /// The caller pins the selected frame, excludes guest access during the
    /// replacement, and retires the alias before invalidating the frame.
    pub unsafe fn replace_page(
        &mut self,
        address: usize,
        backing: Option<(BorrowedFd<'_>, usize, bool)>,
    ) -> io::Result<()> {
        if !self.range.contains(&address) || !address.is_multiple_of(self.page) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let (protection, flags, fd, offset) = match backing {
            Some((file, offset, writable)) => (
                libc::PROT_READ | if writable { libc::PROT_WRITE } else { 0 },
                libc::MAP_SHARED,
                file.as_raw_fd(),
                libc::off_t::try_from(offset).map_err(|_| io::ErrorKind::InvalidInput)?,
            ),
            None => (libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0),
        };
        let result = unsafe {
            libc::mmap(
                ptr::with_exposed_provenance_mut(address),
                self.page,
                protection,
                flags | libc::MAP_FIXED,
                fd,
                offset,
            )
        };
        if result == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for LocalWindow {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(
                ptr::with_exposed_provenance_mut(self.range.start),
                self.range.len(),
            )
        };
    }
}
