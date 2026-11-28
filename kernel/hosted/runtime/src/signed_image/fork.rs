//! Stable kernel aliases for same-address fork siblings on the single iOS carrier.
use super::{Lease, MAX_RESERVED, RESERVED};
use std::{
    io, ptr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

unsafe extern "C" {
    fn vm_remap(
        target_task: libc::mach_port_t,
        target: *mut usize,
        size: usize,
        mask: usize,
        flags: i32,
        source_task: libc::mach_port_t,
        source: usize,
        copy: libc::boolean_t,
        current: *mut i32,
        maximum: *mut i32,
        inheritance: i32,
    ) -> i32;
}

pub(super) struct Backing {
    pub(super) base: usize,
    size: usize,
}

impl Backing {
    fn reserve(size: usize) -> io::Result<Self> {
        RESERVED
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(size)
                    .filter(|&total| total <= MAX_RESERVED)
            })
            .map_err(|_| io::Error::other("Signed fork VA budget exhausted"))?;
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            RESERVED.fetch_sub(size, Ordering::Release);
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            base: base.expose_provenance(),
            size,
        })
    }
}

impl Drop for Backing {
    fn drop(&mut self) {
        unsafe { libc::munmap(ptr::with_exposed_provenance_mut(self.base), self.size) };
        RESERVED.fetch_sub(self.size, Ordering::Release);
    }
}

impl Lease {
    fn mirror(&self, target: usize, source: usize, copy: bool) -> io::Result<()> {
        // Keep signed RX in place. Remap the other ranges including their
        // inaccessible gaps so a sibling cannot inherit a longer heap.
        for range in self
            .image
            .manifest
            .non_executable_ranges(self.image.reservation_size())
        {
            let mut destination = target + range.start;
            let mut current = 0;
            let mut maximum = 0;
            let result = unsafe {
                let task = libc::mach_task_self();
                vm_remap(
                    task,
                    &mut destination,
                    range.len(),
                    0,
                    libc::VM_FLAGS_OVERWRITE,
                    task,
                    source + range.start,
                    copy.into(),
                    &mut current,
                    &mut maximum,
                    libc::VM_INHERIT_NONE,
                )
            };
            if result != 0 {
                return Err(io::Error::other(format!("vm_remap failed ({result})")));
            }
        }
        Ok(())
    }

    pub(crate) fn fork(&self) -> io::Result<Arc<Self>> {
        let size = self.image.reservation_size();
        let mut resident = self.resident.lock().unwrap();
        let parent = if let Some(parent) = self.backing.get() {
            parent
        } else {
            let parent = Backing::reserve(size)?;
            self.mirror(parent.base, self.base, false)?;
            *resident = Some(parent.base);
            self.backing.get_or_init(|| parent)
        };
        let child = Backing::reserve(size)?;
        self.mirror(child.base, parent.base, true)?;
        Ok(Arc::new(Self {
            window: self.window.clone(),
            backing: OnceLock::from(child),
            heap_bytes: AtomicUsize::new(self.heap_bytes.load(Ordering::Acquire)),
        }))
    }

    pub(crate) fn activate(&self) -> io::Result<()> {
        let Some(backing) = self.backing.get() else {
            return Ok(());
        };
        let mut resident = self.resident.lock().unwrap();
        if *resident != Some(backing.base) {
            *resident = None;
            self.mirror(self.base, backing.base, false)?;
            *resident = Some(backing.base);
        }
        Ok(())
    }

    pub(crate) fn kernel_alias(&self, address: usize) -> usize {
        let Some(backing) = self.backing.get() else {
            return address;
        };
        let offset = address - self.base;
        if self
            .image
            .manifest
            .rx_pages()
            .any(|rx| rx.contains(&offset))
        {
            address
        } else {
            backing.base + offset
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(backing) = self.backing.get() {
            let mut resident = self.resident.lock().unwrap();
            if *resident == Some(backing.base) {
                *resident = None;
            }
        }
    }
}
