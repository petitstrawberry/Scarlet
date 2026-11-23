//! File-backed signed text with per-instance mutable pages on iOS.

mod fork;
mod link;
mod manifest;
pub(crate) use link::Namespace;
pub use link::{LoadedLibrary, ObjectId};

use fork::Backing;

use crate::in_process::SignedText;
use manifest::{Access, Kind, Manifest};
use scarlet_abi::tls::{NATIVE_TLS_MAPPING_SIZE, NativeTlsHeader};
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
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

const PAGE: usize = 16384;
// The initial TLS page stays mapped for the process lifetime. Its padding is
// outside the native thread ABI and supplies one address-space-owned word.
const ENVIRON_OFFSET: usize = PAGE - size_of::<usize>();
const _: () = assert!(NATIVE_TLS_MAPPING_SIZE <= ENVIRON_OFFSET);
// Match the ordinary Scarlet executable's initial stack size.
const STACK_SIZE: usize = 1024 * 1024;
// Reserve heap addresses up front; the process commits only what sbrk grows.
const HEAP_CAPACITY: usize = 128 * 1024 * 1024;
// TLS, heap reservation, a stack guard, then the stack.
const STACK_OFFSET: usize = PAGE + HEAP_CAPACITY + PAGE;
const MUTABLE_TAIL: usize = STACK_OFFSET + STACK_SIZE;
const MAX_SLOTS: usize = 8;
// Signed windows and fork mirrors reserve address space for future heap
// growth. Their inaccessible gaps do not consume guest RAM.
const MAX_RESERVED: usize = 32 * 1024 * 1024 * 1024;
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
    constructors: Option<usize>,
    text_bytes: usize,
    writable_bytes: usize,
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

    pub fn startup(&self) -> usize {
        let manifest = &self.lease.image.manifest;
        if manifest.kind == Kind::Pie && self.constructors.is_none() {
            return self.entry();
        }
        self.lease.base + manifest.startup.unwrap_or(manifest.entry)
    }

    /// Address of the signed startup stub's constructor list, when required.
    pub fn constructor_list(&self) -> usize {
        self.constructors.unwrap_or(0)
    }

    pub fn tls(&self) -> usize {
        self.lease.base + self.lease.image.manifest.span
    }

    pub fn environ_slot(&self) -> Option<usize> {
        self.text.namespace().map(|_| self.tls() + ENVIRON_OFFSET)
    }

    pub fn stack_top(&self) -> usize {
        self.constructors.unwrap_or(self.tls() + MUTABLE_TAIL)
    }

    pub fn heap_start(&self) -> usize {
        self.lease.base + self.lease.heap_offset()
    }

    pub fn stack_size(&self) -> usize {
        STACK_SIZE
    }

    pub fn text_size(&self) -> usize {
        self.text_bytes
    }

    pub fn writable_size(&self) -> usize {
        self.writable_bytes
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
    window: Arc<Window>,
    backing: OnceLock<Backing>,
    heap_bytes: AtomicUsize,
}

// Fork siblings retain the original numeric addresses for all guest pointers.
// Only one sibling can execute on the iOS carrier at a time.
pub(crate) struct Window {
    resident: Mutex<Option<usize>>,
    image: Arc<Inner>,
    slot: usize,
    base: usize,
}

impl std::ops::Deref for Lease {
    type Target = Window;

    fn deref(&self) -> &Window {
        &self.window
    }
}

impl Lease {
    pub(crate) fn owns_address(&self, address: usize) -> bool {
        address
            .checked_sub(self.base)
            .is_some_and(|offset| offset < self.image.reservation_size())
    }

    fn heap_offset(&self) -> usize {
        self.image.manifest.span + PAGE
    }

    pub(crate) fn writable_image_size(&self) -> usize {
        self.image
            .manifest
            .segments
            .iter()
            .filter(|segment| segment.access == Access::ReadWrite)
            .map(|segment| segment.pages().len())
            .sum()
    }

    pub(crate) fn committed_data_size(&self) -> usize {
        self.writable_image_size() + self.heap_bytes.load(Ordering::Acquire)
    }

    pub(crate) fn executable_image_size(&self) -> usize {
        self.image.manifest.rx_pages().map(|range| range.len()).sum()
    }

    pub(crate) fn resize_heap(&self, address: usize, limit: usize) -> io::Result<usize> {
        let start = self.heap_offset();
        let bytes = address
            .checked_sub(self.base + start)
            .filter(|&bytes| bytes <= HEAP_CAPACITY)
            .ok_or_else(|| {
                io::Error::other("Program break is outside the signed heap reservation")
            })?;
        let rounded = manifest::align_up(bytes);
        let new_data = self.writable_image_size() + rounded;
        if new_data > limit {
            return Err(io::Error::other(
                "Program break exceeds the task data limit",
            ));
        }
        let previous = self.heap_bytes.load(Ordering::Acquire);
        let base = self.backing.get().map_or(self.base, |backing| backing.base);
        if base != self.base && rounded != previous {
            *self.resident.lock().unwrap() = None;
        }
        if rounded > previous {
            map_at(
                base,
                start + previous..start + rounded,
                libc::PROT_READ | libc::PROT_WRITE,
                None,
            )?;
        } else if rounded < previous {
            map_at(
                base,
                start + rounded..start + previous,
                libc::PROT_NONE,
                None,
            )?;
        }
        self.heap_bytes.store(rounded, Ordering::Release);
        Ok(new_data)
    }

    pub(crate) fn user_bounds(&self, address: usize, write: bool) -> Option<Range<usize>> {
        let offset = address.checked_sub(self.base)?;
        let manifest = &self.image.manifest;
        let end = manifest.span;
        let heap = self.heap_offset();
        let tail = [
            end..end + PAGE,
            heap..heap + self.heap_bytes.load(Ordering::Acquire),
            end + STACK_OFFSET..end + MUTABLE_TAIL,
        ];
        let pages = if let Some(bounds) = tail
            .into_iter()
            .find(|bounds| manifest.kind != Kind::Dso && bounds.contains(&offset))
        {
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
                for relro in &manifest.relro {
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

impl Drop for Window {
    fn drop(&mut self) {
        let mut pool = self.image.pool.lock().unwrap();
        let slot = &mut pool.slots[self.slot];
        let total = self.image.reservation_size();
        let reset = self
            .image
            .manifest
            .non_executable_ranges(total)
            .try_for_each(|range| map_at(self.base, range, libc::PROT_NONE, None));
        slot.state = if reset.is_ok() {
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

    pub fn dependencies(&self) -> &[(String, String)] {
        &self.0.manifest.dependencies
    }

    pub fn instantiate(&self) -> io::Result<Instance> {
        if self.0.manifest.kind != Kind::Static {
            return Err(io::Error::other("Dynamic ELF needs a process link context"));
        }
        let lease = self.0.acquire()?;
        self.0.initialize_segments(lease.base)?;
        self.0.finish_instance(lease.base)?;
        Instance::from_modules(vec![lease], Vec::new(), None)
    }
}

impl Instance {
    fn from_modules(
        leases: Vec<Arc<Lease>>,
        mut constructors: Vec<usize>,
        namespace: Option<Namespace>,
    ) -> io::Result<Self> {
        let lease = leases
            .first()
            .ok_or_else(|| io::Error::other("Missing main executable"))?
            .clone();
        let (text, text_bytes, writable_bytes) = linked_text(leases, namespace)?;
        let constructors = if constructors.is_empty() {
            None
        } else {
            constructors.extend([0, lease.base + lease.image.manifest.entry]);
            let bytes = (constructors.len() * size_of::<usize>()).next_multiple_of(16);
            if bytes >= STACK_SIZE {
                return Err(io::Error::other(
                    "Constructor list exceeds the process stack",
                ));
            }
            let address = lease.base + lease.image.reservation_size() - bytes;
            // The unpublished main owns the writable stack and no guest has run.
            unsafe {
                ptr::copy_nonoverlapping(
                    constructors.as_ptr(),
                    ptr::with_exposed_provenance_mut(address),
                    constructors.len(),
                )
            };
            Some(address)
        };
        Ok(Self {
            text,
            lease,
            constructors,
            text_bytes,
            writable_bytes,
        })
    }
}

fn linked_text(
    leases: Vec<Arc<Lease>>,
    namespace: Option<Namespace>,
) -> io::Result<(Arc<SignedText>, usize, usize)> {
    let mut ranges = Vec::new();
    let mut patches = Vec::new();
    let mut text_bytes = 0;
    let mut writable_bytes = 0;
    for module in &leases {
        let base = module.base;
        writable_bytes += module.writable_image_size();
        for range in module.image.manifest.rx_pages() {
            text_bytes += range.len();
            ranges.push((base + range.start) as u64..(base + range.end) as u64);
        }
        patches.extend(
            module
                .image
                .manifest
                .patches
                .iter()
                .map(|&(pc, patch)| ((base + pc) as u64, patch)),
        );
    }
    if namespace.is_some() {
        SignedText::include_link_gate(&mut ranges, &mut patches);
    }
    ranges.sort_unstable_by_key(|range| range.start);
    // Each lease pins its own immutable signed RX ranges.
    let mut text = unsafe { SignedText::from_ranges(ranges, patches)? };
    text.retain_mappings(leases, namespace);
    Ok((Arc::new(text), text_bytes, writable_bytes))
}

impl Inner {
    fn reservation_size(&self) -> usize {
        self.manifest.span
            + if self.manifest.kind == Kind::Dso {
                0
            } else {
                MUTABLE_TAIL
            }
    }

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
            window: Arc::new(Window {
                image: self.clone(),
                slot,
                base: pool.slots[slot].base,
                resident: Mutex::new(None),
            }),
            backing: OnceLock::new(),
            heap_bytes: AtomicUsize::new(0),
        }))
    }

    fn reserve(&self) -> io::Result<usize> {
        let size = self.reservation_size();
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
        let mut file_offset = self.rx.start;
        for rx in self.manifest.rx_pages() {
            let length = rx.len();
            if let Err(error) = map_at(
                base_address,
                rx,
                libc::PROT_READ | libc::PROT_EXEC,
                Some((&self.file, file_offset)),
            ) {
                // Darwin may retain an already mapped signed range. Account
                // the failed reservation permanently if any RX was installed.
                if file_offset == self.rx.start {
                    unsafe { libc::munmap(base, size) };
                    RESERVED.fetch_sub(size, Ordering::Release);
                }
                return Err(io::Error::other(format!(
                    "mmap signed executable segment: {error}"
                )));
            }
            file_offset += length;
        }
        Ok(base_address)
    }

    fn initialize_segments(&self, base: usize) -> io::Result<()> {
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
        Ok(())
    }

    fn finish_instance(&self, base: usize) -> io::Result<()> {
        let manifest = &self.manifest;
        for segment in &manifest.segments {
            if segment.access == Access::ReadOnly {
                protect(base, segment.pages(), libc::PROT_READ)?;
            }
        }
        for relro in &manifest.relro {
            protect(base, relro.clone(), libc::PROT_READ)?;
        }
        if manifest.kind == Kind::Dso {
            return Ok(());
        }
        let end = manifest.span;
        let rw = libc::PROT_READ | libc::PROT_WRITE;
        let tls = base + end;
        map_at(base, end..end + PAGE, rw, None)?;
        map_at(base, end + STACK_OFFSET..end + MUTABLE_TAIL, rw, None)?;
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
