//! File-backed signed text with per-instance mutable pages on iOS.

mod manifest;

use crate::in_process::SignedText;
use manifest::{Access, Manifest};
use scarlet_abi::tls::NativeTlsHeader;
use std::{
    ffi::{CStr, CString, c_void},
    fs::File,
    io,
    ops::Range,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::FileExt},
    },
    path::Path,
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

const PAGE: usize = 16384;
const STACK_SIZE: usize = 4 * PAGE;
// TLS, initial heap, a stack guard and the stack.
const MUTABLE_TAIL: usize = 3 * PAGE + STACK_SIZE;
const MAX_SLOTS: usize = 8;
const MAX_RESERVED: usize = 256 * 1024 * 1024;
static RESERVED: AtomicUsize = AtomicUsize::new(0);

struct Library {
    handle: usize,
    // Once published, Darwin may keep file-backed RX mappings indefinitely.
    resident: bool,
}

impl Drop for Library {
    fn drop(&mut self) {
        if !self.resident {
            unsafe { libc::dlclose(self.handle as *mut c_void) };
        }
    }
}

impl Library {
    fn open(path: &Path) -> io::Result<Self> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        let handle = unsafe {
            libc::dlopen(
                path.as_ptr(),
                libc::RTLD_NOW | libc::RTLD_LOCAL | libc::RTLD_FIRST,
            )
        };
        if handle.is_null() {
            let error = unsafe { libc::dlerror() };
            let message = if error.is_null() {
                "unknown error".into()
            } else {
                unsafe { CStr::from_ptr(error) }.to_string_lossy()
            };
            return Err(io::Error::other(format!("dlopen signed image: {message}")));
        }
        Ok(Self {
            handle: handle.addr(),
            resident: false,
        })
    }

    fn symbol(&self, name: &CStr) -> io::Result<usize> {
        let value = unsafe { libc::dlsym(self.handle as *mut c_void, name.as_ptr()) };
        if value.is_null() {
            return Err(io::Error::other(format!(
                "Missing signed image symbol {name:?}"
            )));
        }
        Ok(value.addr())
    }

    fn file_range(&self, start: &CStr, end: &CStr, file_size: usize) -> io::Result<Range<usize>> {
        let start = self.symbol(start)?;
        let end = self.symbol(end)?;
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        if unsafe { libc::dladdr(ptr::with_exposed_provenance(start), &mut info) } == 0 {
            return Err(io::Error::other("Cannot locate signed image in dyld"));
        }
        let base = info.dli_fbase.addr();
        let (Some(start), Some(end)) = (start.checked_sub(base), end.checked_sub(base)) else {
            return Err(io::Error::other(
                "Signed image symbol precedes its Mach-O header",
            ));
        };
        if start > end || end > file_size {
            return Err(io::Error::other(
                "Signed image payload is outside file-backed __TEXT",
            ));
        }
        Ok(start..end)
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum State {
    Idle,
    Live,
    Unavailable,
}

struct Slot {
    base: usize,
    state: State,
}

struct Pool {
    library: Library,
    slots: Vec<Slot>,
}

struct Inner {
    pool: Mutex<Pool>,
    file: File,
    rx: Range<usize>,
    template: Vec<u8>,
    manifest: Manifest,
}

pub struct Image(Arc<Inner>);

pub struct Instance {
    text: Arc<SignedText>,
    lease: Arc<Lease>,
}

pub struct TlsTemplate {
    pub address: usize,
    pub file_size: usize,
    pub memory_size: usize,
    pub alignment: usize,
}

impl Instance {
    pub fn text(&self) -> Arc<SignedText> {
        self.text.clone()
    }

    pub fn entry(&self) -> usize {
        self.lease.base + self.lease.image.manifest.entry
    }

    pub fn tls(&self) -> usize {
        self.lease.base + self.lease.image.manifest.span
    }

    pub fn stack_top(&self) -> usize {
        self.tls() + MUTABLE_TAIL
    }

    pub fn stack_size(&self) -> usize {
        STACK_SIZE
    }

    pub fn text_size(&self) -> usize {
        self.lease.image.manifest.rx_pages().len()
    }

    pub fn writable_size(&self) -> usize {
        self.lease
            .image
            .manifest
            .segments
            .iter()
            .filter(|segment| segment.access == Access::ReadWrite)
            .map(|segment| segment.pages().len())
            .sum()
    }

    pub fn program_headers(&self) -> Option<(usize, usize)> {
        let (address, count) = self.lease.image.manifest.phdr?;
        Some((self.lease.base + address, count))
    }

    pub fn tls_template(&self) -> Option<TlsTemplate> {
        let template = self.lease.image.manifest.tls.as_ref()?;
        Some(TlsTemplate {
            address: self.lease.base + template.template.start,
            file_size: template.template.len(),
            memory_size: template.memory_size,
            alignment: template.alignment,
        })
    }
}

pub(crate) struct Lease {
    image: Arc<Inner>,
    slot: usize,
    base: usize,
}

impl Lease {
    pub(crate) fn user_bounds(&self, address: usize, write: bool) -> Option<Range<usize>> {
        let offset = address.checked_sub(self.base)?;
        let manifest = &self.image.manifest;
        let end = manifest.span;
        let tail = [end..end + 2 * PAGE, end + 3 * PAGE..end + MUTABLE_TAIL];
        let pages = if let Some(bounds) = tail.into_iter().find(|bounds| bounds.contains(&offset)) {
            bounds
        } else {
            let (segment, mut bounds) = manifest.segments.iter().find_map(|segment| {
                let bounds = segment.pages();
                bounds.contains(&offset).then_some((segment, bounds))
            })?;
            if write {
                if segment.access != Access::ReadWrite {
                    return None;
                }
                if let Some(relro) = &manifest.relro {
                    if relro.contains(&offset) {
                        return None;
                    }
                    if bounds.contains(&relro.start) && offset < relro.start {
                        bounds.end = relro.start;
                    }
                    if bounds.contains(&relro.end) && offset >= relro.end {
                        bounds.start = relro.end;
                    }
                }
            }
            bounds
        };
        Some(self.base + pages.start..self.base + pages.end)
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut pool = self.image.pool.lock().unwrap();
        let slot = &mut pool.slots[self.slot];
        let rx = self.image.manifest.rx_pages();
        let total = self.image.manifest.span + MUTABLE_TAIL;
        let reset = |range| map_at(self.base, range, libc::PROT_NONE, None);
        // Never replace the executable or return its virtual range to Darwin.
        let before = reset(0..rx.start);
        let after = reset(rx.end..total);
        slot.state = if before.is_ok() && after.is_ok() {
            State::Idle
        } else {
            State::Unavailable
        };
    }
}

impl Image {
    /// Open an app-bundled Mach-O produced by the Scarlet AOT tool.
    ///
    /// # Safety
    /// The caller must supply an immutable, trusted app bundle image whose
    /// executable bytes have been reviewed for direct in-process execution.
    pub unsafe fn open(path: &Path) -> io::Result<Self> {
        if crate::ram::host_page_size()? != PAGE {
            return Err(io::Error::other("Signed images require 16 KiB host pages"));
        }
        let file = File::open(path)?;
        let file_size = text_file_size(&file)?;
        let library = Library::open(path)?;
        let rx = library.file_range(c"scarlet_aot_rx", c"scarlet_aot_rx_end", file_size)?;
        let data = library.file_range(c"scarlet_aot_data", c"scarlet_aot_data_end", file_size)?;
        let metadata = library.file_range(
            c"scarlet_aot_manifest",
            c"scarlet_aot_manifest_end",
            file_size,
        )?;
        if !rx.start.is_multiple_of(PAGE)
            || !rx.len().is_multiple_of(PAGE)
            || rx.end > data.start
            || data.end > metadata.start
            || data.len() > 64 * 1024 * 1024
            || metadata.len() > 4 * 1024 * 1024
        {
            return Err(io::Error::other("Invalid signed Mach-O payload layout"));
        }
        let mut bytes = vec![0; metadata.len()];
        file.read_exact_at(&mut bytes, metadata.start as u64)?;
        let metadata = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
        let manifest = Manifest::parse(metadata, data.len(), rx.len())?;
        let mut template = vec![0; data.len()];
        file.read_exact_at(&mut template, data.start as u64)?;
        Ok(Self(Arc::new(Inner {
            pool: Mutex::new(Pool {
                library,
                slots: Vec::new(),
            }),
            file,
            rx,
            template,
            manifest,
        })))
    }

    pub fn elf_sha256(&self) -> &str {
        &self.0.manifest.hash
    }

    pub fn instantiate(&self) -> io::Result<Instance> {
        let inner = &self.0;
        let lease = inner.acquire()?;
        let base = lease.base;
        let manifest = &inner.manifest;
        let tls = base + manifest.span;
        inner.initialize(base, tls)?;
        let rx = manifest.rx_pages();
        let patches = manifest
            .patches
            .iter()
            .map(|&(pc, patch)| ((base + pc) as u64, patch));
        // SAFETY: This lease pins the exact immutable file-backed RX range.
        let mut text =
            unsafe { SignedText::new((base + rx.start) as u64..(base + rx.end) as u64, patches)? };
        text.retain_mapping(lease.clone());
        Ok(Instance {
            text: Arc::new(text),
            lease,
        })
    }
}

impl Inner {
    fn acquire(self: &Arc<Self>) -> io::Result<Arc<Lease>> {
        let mut pool = self.pool.lock().unwrap();
        let slot = if let Some(slot) = pool.slots.iter().position(|slot| slot.state == State::Idle)
        {
            slot
        } else {
            if pool.slots.len() == MAX_SLOTS {
                return Err(io::Error::other("Signed image pool is full"));
            }
            let base = self.reserve()?;
            pool.library.resident = true;
            pool.slots.push(Slot {
                base,
                state: State::Idle,
            });
            pool.slots.len() - 1
        };
        pool.slots[slot].state = State::Live;
        Ok(Arc::new(Lease {
            image: self.clone(),
            slot,
            base: pool.slots[slot].base,
        }))
    }

    fn reserve(&self) -> io::Result<usize> {
        let size = self.manifest.span + MUTABLE_TAIL;
        RESERVED
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(size).filter(|&used| used <= MAX_RESERVED)
            })
            .map_err(|_| io::Error::other("Signed guest VA budget exhausted"))?;
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
        let base_address = base.expose_provenance();
        let rx = self.manifest.rx_pages();
        if let Err(error) = map_at(
            base_address,
            rx,
            libc::PROT_READ | libc::PROT_EXEC,
            Some((&self.file, self.rx.start)),
        ) {
            unsafe { libc::munmap(base, size) };
            RESERVED.fetch_sub(size, Ordering::Release);
            return Err(io::Error::other(format!(
                "mmap signed executable segment: {error}"
            )));
        }
        Ok(base_address)
    }

    fn initialize(&self, base: usize, tls: usize) -> io::Result<()> {
        let manifest = &self.manifest;
        let rw = libc::PROT_READ | libc::PROT_WRITE;
        for segment in &manifest.segments {
            if segment.access == Access::Execute {
                continue;
            }
            map_at(base, segment.pages(), rw, None)?;
            let destination = ptr::with_exposed_provenance_mut::<u8>(base + segment.address);
            let bytes = &self.template[segment.source.clone()];
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len()) };
        }
        for &(destination, value) in &manifest.relocs {
            let location = ptr::with_exposed_provenance_mut::<usize>(base + destination);
            unsafe { location.write(base + value) };
        }
        for segment in &manifest.segments {
            if segment.access == Access::ReadOnly {
                protect(base, segment.pages(), libc::PROT_READ)?;
            }
        }
        if let Some(relro) = &manifest.relro {
            protect(base, relro.clone(), libc::PROT_READ)?;
        }
        let end = manifest.span;
        map_at(base, end..end + 2 * PAGE, rw, None)?;
        map_at(base, end + 3 * PAGE..end + MUTABLE_TAIL, rw, None)?;
        unsafe {
            ptr::with_exposed_provenance_mut::<NativeTlsHeader>(tls).write(NativeTlsHeader::INITIAL)
        };
        if let Some(template) = &manifest.tls {
            let from = ptr::with_exposed_provenance::<u8>(base + template.template.start);
            let to = ptr::with_exposed_provenance_mut::<u8>(tls + template.offset);
            unsafe { ptr::copy_nonoverlapping(from, to, template.template.len()) };
        }
        Ok(())
    }
}

fn map_at(
    base: usize,
    range: Range<usize>,
    prot: i32,
    file: Option<(&File, usize)>,
) -> io::Result<()> {
    if range.is_empty() {
        return Ok(());
    }
    let (flag, fd, offset) = file.map_or((libc::MAP_ANON, -1, 0), |(file, offset)| {
        (0, file.as_raw_fd(), offset as libc::off_t)
    });
    let address = ptr::with_exposed_provenance_mut(base + range.start);
    // All callers own the reservation and have excluded any running guest.
    let mapped = unsafe {
        libc::mmap(
            address,
            range.len(),
            prot,
            flag | libc::MAP_PRIVATE | libc::MAP_FIXED,
            fd,
            offset,
        )
    };
    if mapped == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn protect(base: usize, range: Range<usize>, prot: i32) -> io::Result<()> {
    let address = ptr::with_exposed_provenance_mut(base + range.start);
    if unsafe { libc::mprotect(address, range.len(), prot) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn text_file_size(file: &File) -> io::Result<usize> {
    let mut header = [0; 32];
    file.read_exact_at(&mut header, 0)?;
    let u32_at = |bytes: &[u8], off| u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
    let u64_at = |bytes: &[u8], off| u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
    if u32_at(&header, 0) != 0xfeed_facf
        || u32_at(&header, 4) != 0x0100_000c
        || u32_at(&header, 12) != 6
    {
        return Err(io::Error::other("Expected a thin arm64 Mach-O dylib"));
    }
    let command_size = u32_at(&header, 20) as usize;
    if command_size > 64 * 1024 {
        return Err(io::Error::other("Mach-O load commands are too large"));
    }
    let mut commands = vec![0; command_size];
    file.read_exact_at(&mut commands, 32)?;
    let mut commands = commands.as_slice();
    for _ in 0..u32_at(&header, 16) {
        if commands.len() < 8 {
            break;
        }
        let len = u32_at(commands, 4) as usize;
        if len < 8 || len > commands.len() {
            break;
        }
        if u32_at(commands, 0) == 0x19
            && len >= 72
            && &commands[8..24] == b"__TEXT\0\0\0\0\0\0\0\0\0\0"
        {
            let offset = u64_at(commands, 40);
            let size = u64_at(commands, 48);
            if offset == 0
                && size <= file.metadata()?.len()
                && size <= u64_at(commands, 32)
                && u32_at(commands, 60) == (libc::PROT_READ | libc::PROT_EXEC) as u32
            {
                return Ok(size as usize);
            }
            break;
        }
        commands = &commands[len..];
    }
    Err(io::Error::other("Invalid signed Mach-O __TEXT segment"))
}
