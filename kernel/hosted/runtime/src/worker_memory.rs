//! Installation of an acknowledged mapping in the native worker's VM.

use std::{
    ffi::c_void,
    io,
    ops::Range,
    os::fd::{AsRawFd, BorrowedFd},
    ptr::{self, NonNull},
};

use crate::{fetched_instruction::shadow::Shadows, ram::host_page_size};

use crate::platform_vm::{limit_permissions, reserve, synchronize_instructions};

#[derive(Clone, Copy, Debug)]
pub enum Protection<'a> {
    ReadOnly,
    ReadWrite,
    /// Decoded instruction offsets in the supplied extent. AArch64 literal
    /// pools and ELF data ranges must be excluded by the image publisher.
    ReadExecute(&'a [Range<usize>]),
    /// Fetchable guest intervals with unknown code/data layout. Preserve the
    /// original bytes at the guest VA and publish instructions on actual fetch.
    ReadExecuteOnFetch(&'a [Range<usize>]),
    /// Fetch through the snapshot, but fault all guest data accesses.
    ExecuteOnFetch(&'a [Range<usize>]),
}

impl Protection<'_> {
    fn native(self) -> libc::c_int {
        match self {
            Self::ExecuteOnFetch(_) => libc::PROT_NONE,
            Self::ReadOnly | Self::ReadExecuteOnFetch(_) => libc::PROT_READ,
            Self::ReadWrite => libc::PROT_READ | libc::PROT_WRITE,
            Self::ReadExecute(_) => libc::PROT_READ | libc::PROT_EXEC,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchedInstruction {
    Syscall { immediate: u16 },
    ReadThreadPointer { register: u8 },
    WriteThreadPointer { register: u8 },
    PlatformRegister(crate::platform_register::Action),
}

pub(crate) const SYSCALL_BREAKPOINT: u32 = 0xd420_0000 | (0x5300 << 5);

pub(super) fn svc_immediate(word: u32) -> Option<u16> {
    (word & 0xffe0_001f == 0xd400_0001).then_some((word >> 5) as u16)
}

#[derive(Debug)]
struct Patch {
    offset: usize,
    original: u32,
    replacement: u32,
    instruction: PatchedInstruction,
}

#[cfg(target_os = "macos")]
mod canonical_x18;

#[cfg(target_os = "macos")]
use crate::platform_register::plan as plan_platform_register;

#[cfg(target_os = "linux")]
fn plan_platform_register(_: u32, _: &mut Vec<u32>) -> Option<crate::platform_register::Action> {
    // The Linux AArch64 ABI leaves x18 available to the guest.
    None
}

fn patch_instructions(
    bytes: &mut [u8],
    ranges: &[Range<usize>],
    steps: &mut Vec<u32>,
) -> io::Result<Vec<Patch>> {
    use PatchedInstruction::*;

    let mut patches = Vec::new();
    let mut previous_end = 0;
    for range in ranges {
        if range.start < previous_end
            || range.start >= range.end
            || range.end > bytes.len()
            || !range.start.is_multiple_of(4)
            || !range.end.is_multiple_of(4)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid guest instruction range",
            ));
        }
        for (index, word) in bytes[range.clone()].chunks_exact(4).enumerate() {
            let original = u32::from_le_bytes(word.try_into().unwrap());
            let register = (original & 31) as u8;
            let (replacement, instruction) = if let Some(immediate) = svc_immediate(original) {
                (SYSCALL_BREAKPOINT, Syscall { immediate })
            } else if original & !31 == 0xd53b_d040 {
                (0xd420_0000 | (0x5400 << 5), ReadThreadPointer { register })
            } else if original & !31 == 0xd51b_d040 {
                (0xd420_0000 | (0x5500 << 5), WriteThreadPointer { register })
            } else if let Some(action) = plan_platform_register(original, steps) {
                (0xd420_0000 | (0x5700 << 5), PlatformRegister(action))
            } else {
                continue;
            };
            patches.push(Patch {
                offset: range.start + index * 4,
                original,
                replacement,
                instruction,
            });
        }
        previous_end = range.end;
    }
    #[cfg(target_os = "macos")]
    canonical_x18::plan(bytes, ranges, &mut patches, steps);
    for patch in &patches {
        bytes[patch.offset..patch.offset + 4].copy_from_slice(&patch.replacement.to_le_bytes());
    }
    Ok(patches)
}

/// Private, immutable worker instructions. No writable alias survives sealing.
#[derive(Debug)]
pub(super) struct NativeSteps {
    base: NonNull<c_void>,
    length: usize,
}
impl NativeSteps {
    pub(super) fn address(&self) -> u64 {
        self.base.as_ptr() as u64
    }

    pub(super) fn seal(words: &[u32]) -> io::Result<Option<Self>> {
        if words.is_empty() {
            return Ok(None);
        }
        let size = size_of_val(words);
        let length = size
            .checked_next_multiple_of(host_page_size()?)
            .ok_or(io::ErrorKind::InvalidInput)?;
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let steps = Self {
            base: NonNull::new(base).expect("Nonzero native mapping"),
            length,
        };
        unsafe {
            ptr::copy_nonoverlapping(words.as_ptr(), base.cast(), words.len());
            synchronize_instructions(base, size);
            let rx = libc::PROT_READ | libc::PROT_EXEC;
            if libc::mprotect(base, length, rx) != 0 {
                return Err(io::Error::last_os_error());
            }
            limit_permissions(base, length, rx)?;
        }
        Ok(Some(steps))
    }
}
impl Drop for NativeSteps {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.base.as_ptr(), self.length);
        }
    }
}

#[derive(Debug)]
struct FetchInterval {
    start: usize,
    words: Box<[u32]>,
    shadows: Shadows,
}

/// A live range owned exclusively by the worker's address-space manager.
#[derive(Debug)]
pub struct Mapping {
    publication: u64,
    base: NonNull<c_void>,
    len: usize,
    patches: Vec<Patch>,
    executable: bool,
    native_text: Box<[Range<usize>]>,
    steps: Option<NativeSteps>,
    fetch: Vec<FetchInterval>,
}

// The allocation is process-wide. Moving the owner between threads does
// not move its pages. Shared methods only read sealed instructions and
// immutable metadata. The worker takes the registry write lock for removal.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Install an exact guest range from an already published backing extent.
    ///
    /// # Safety
    /// The backing must cover the complete aligned extent and remain valid
    /// while mapped. The publisher must keep executable bytes immutable
    /// while they are copied. Guest threads may only access the range after
    /// installation succeeds and must acknowledge a stop before it is dropped.
    pub unsafe fn install(
        address: usize,
        len: usize,
        backing: BorrowedFd<'_>,
        offset: usize,
        protection: Protection<'_>,
    ) -> io::Result<Self> {
        let page = host_page_size()?;
        if !offset.is_multiple_of(page) || offset.checked_add(len).is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid backing offset"));
        }
        let file_offset = libc::off_t::try_from(offset).map_err(|_| io::ErrorKind::InvalidInput)?;
        let mut mapping = Self::reserve(address, len)?;
        mapping.executable = matches!(protection, Protection::ReadExecute(_));
        let native = protection.native();
        if matches!(
            protection,
            Protection::ReadExecute(_)
                | Protection::ReadExecuteOnFetch(_)
                | Protection::ExecuteOnFetch(_)
        ) {
            // Darwin POSIX shared memory is nonexecutable. Install an immutable
            // text snapshot in a private mapping, transitioning directly from RW to RX.
            let source = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    len,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    backing.as_raw_fd(),
                    file_offset,
                )
            };
            if source == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            let Some(source_base) = NonNull::new(source) else {
                unsafe { libc::munmap(source, len) };
                return Err(io::Error::other("Text source mapped at address zero"));
            };
            let source = Self {
                publication: 0,
                base: source_base,
                len,
                patches: Vec::new(),
                steps: None,
                fetch: Vec::new(),
                executable: false,
                native_text: Box::default(),
            };
            match protection {
                Protection::ReadExecute(instructions) => unsafe {
                    ptr::copy_nonoverlapping(
                        source_base.as_ptr().cast::<u8>(),
                        mapping.base.as_ptr().cast(),
                        len,
                    );
                    let bytes = std::slice::from_raw_parts_mut(mapping.base.as_ptr().cast(), len);
                    let mut words = Vec::new();
                    mapping.patches = patch_instructions(bytes, instructions, &mut words)?;
                    mapping.steps = NativeSteps::seal(&words)?;
                    mapping.native_text = instructions.into();
                    synchronize_instructions(mapping.base.as_ptr(), len);
                },
                Protection::ReadExecuteOnFetch(ranges) | Protection::ExecuteOnFetch(ranges) => {
                    let mut end = 0;
                    for r in ranges {
                        if r.start < end
                            || r.is_empty()
                            || r.end > len
                            || r.start % 4 != 0
                            || r.end % 4 != 0
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "Invalid fetch interval",
                            ));
                        }
                        let words = unsafe {
                            std::slice::from_raw_parts(
                                source.base.as_ptr().cast::<u32>().add(r.start / 4),
                                r.len() / 4,
                            )
                        };
                        mapping.fetch.push(FetchInterval {
                            start: r.start,
                            words: words.into(),
                            shadows: Shadows::new(words.len()),
                        });
                        end = r.end;
                    }
                }
                _ => unreachable!(),
            }
        }
        if matches!(
            protection,
            Protection::ReadExecute(_) | Protection::ExecuteOnFetch(_)
        ) {
            if unsafe { libc::mprotect(mapping.base.as_ptr(), len, native) } != 0 {
                return Err(io::Error::last_os_error());
            }
        } else {
            // SAFETY: MAP_FIXED replaces only the reservation owned by mapping.
            let address = unsafe {
                libc::mmap(
                    mapping.base.as_ptr(),
                    len,
                    native,
                    libc::MAP_SHARED | libc::MAP_FIXED,
                    backing.as_raw_fd(),
                    file_offset,
                )
            };
            if address == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
        }
        limit_permissions(mapping.base.as_ptr(), len, native)?;
        Ok(mapping)
    }

    // The checked access path resolves data from the original guest backing.
    // Nothing from that backing is accessible at this worker address.
    pub(super) fn guard(address: usize, len: usize) -> io::Result<Self> {
        let mapping = Self::reserve(address, len)?;
        if unsafe { libc::mprotect(mapping.base.as_ptr(), len, libc::PROT_NONE) } != 0 {
            return Err(io::Error::last_os_error());
        }
        limit_permissions(mapping.base.as_ptr(), len, libc::PROT_NONE)?;
        Ok(mapping)
    }

    fn reserve(address: usize, len: usize) -> io::Result<Self> {
        let page = host_page_size()?;
        if address == 0 || len == 0 || !address.is_multiple_of(page)
            || !len.is_multiple_of(page) || address.checked_add(len).is_none()
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid worker range"));
        }
        let reserved = reserve(address, len)?;
        static NEXT_PUBLICATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Ok(Self {
            publication: NEXT_PUBLICATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            base: NonNull::new(reserved)
                .expect("The reserved address was nonzero"),
            len,
            patches: Vec::new(),
            steps: None,
            fetch: Vec::new(),
            executable: false,
            native_text: Box::default(),
        })
    }

    pub(super) fn fetch_word(&self, pc: usize) -> Option<u32> {
        self.fetch_words(pc)?.first().copied()
    }

    pub(super) fn fetch_words(&self, pc: usize) -> Option<&[u32]> {
        let (interval, index) = self.fetch_interval(pc)?;
        Some(&interval.words[index..])
    }

    fn fetch_interval(&self, pc: usize) -> Option<(&FetchInterval, usize)> {
        let offset = pc.checked_sub(self.address())?;
        if !offset.is_multiple_of(4) {
            return None;
        }
        let i = self
            .fetch
            .partition_point(|r| r.start <= offset)
            .checked_sub(1)?;
        let interval = &self.fetch[i];
        let start = (offset - interval.start) / 4;
        (start < interval.words.len()).then_some((interval, start))
    }

    // A tail thunk changes only PC. Resolve it against this publication even
    // when the immutable target lives in a different tile or fetch interval.
    fn fetch_entry(&self, pc: usize) -> Option<(&FetchInterval, usize)> {
        let (range, index) = self.fetch_interval(pc)?;
        let word = range.words[index];
        if word >> 26 == 5 {
            let offset = crate::fetched_instruction::branch_offset(word);
            if let Some(entry) = self.fetch_interval(pc.wrapping_add_signed(offset as isize)) {
                return Some(entry);
            }
        }
        Some((range, index))
    }

    pub(super) fn shadow_entry(&self, pc: usize) -> io::Result<Option<u64>> {
        let Some((interval, index)) = self.fetch_entry(pc) else {
            return Ok(None);
        };
        let guest_base = (self.address() + interval.start) as u64;
        interval.shadows.entry(&interval.words, index, guest_base)
    }

    #[cfg(any(target_os = "macos", test))]
    pub(super) fn cached_shadow_entry(&self, pc: usize) -> Option<u64> {
        let (interval, index) = self.fetch_entry(pc)?;
        interval.shadows.cached_entry(index)
    }

    pub(super) fn original_shadow_pc(&self, address: u64) -> Option<u64> {
        self.fetch.iter().find_map(|r| {
            let offset = r.shadows.original_offset(address)?;
            Some((self.address() + r.start + offset) as u64)
        })
    }

    /// Only declared native instruction intervals may populate a linked copy.
    pub(super) fn native_text_word(&self, pc: usize) -> Option<u32> {
        let offset = pc.checked_sub(self.address())?;
        let i = self.native_text.partition_point(|range| range.end <= offset);
        self.native_text.get(i)?.contains(&offset).then(|| self.published_word(pc))?
    }

    pub(super) fn publication(&self) -> u64 { self.publication }

    pub(super) fn contains(&self, pc: usize) -> bool {
        pc.wrapping_sub(self.address()) < self.len
    }

    #[cfg(target_os = "macos")]
    pub(super) fn step_address(&self, slot: usize) -> u64 {
        let steps = self.steps.as_ref().expect("Native step without backing");
        debug_assert!((slot + 2) * 4 <= steps.length);
        steps.base.as_ptr() as u64 + (slot as u64) * 4
    }

    pub fn address(&self) -> usize {
        self.base.as_ptr().addr()
    }

    pub(super) fn instruction_at(&self, pc: usize) -> Option<u32> {
        let offset = pc.checked_sub(self.address())?;
        if !self.executable || !offset.is_multiple_of(4) || offset.checked_add(4)? > self.len {
            return None;
        }
        // SAFETY: A live executable mapping is readable and immutable.
        Some(unsafe { self.base.as_ptr().cast::<u32>().add(offset / 4).read() })
    }

    /// The guest instruction from this publication, including the original
    /// word when native text contains a worker breakpoint in its place.
    pub(super) fn published_word(&self, pc: usize) -> Option<u32> {
        if let Some(word) = self.fetch_word(pc) {
            return Some(word);
        }
        let word = self.instruction_at(pc)?;
        Some(self.patch_at(pc, word).map_or(word, |patch| patch.original))
    }

    /// Decode a stop in this live mapping of the admitted generation.
    pub fn patched_instruction(&self, pc: usize, word: u32) -> Option<PatchedInstruction> {
        Some(self.patch_at(pc, word)?.instruction)
    }

    fn patch_at(&self, pc: usize, word: u32) -> Option<&Patch> {
        let offset = pc.checked_sub(self.address())?;
        let index = self
            .patches
            .binary_search_by_key(&offset, |p| p.offset)
            .ok()?;
        let patch = &self.patches[index];
        (patch.replacement == word).then_some(patch)
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: Installation reserves this full range exclusively and the
        // worker VM contract requires all guest users to have stopped.
        unsafe { libc::munmap(self.base.as_ptr(), self.len) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ram::RamArena;

    #[test]
    fn exact_guest_ranges_share_data_and_execute_sealed_text() {
        const IMAGE: usize = 0x78_0000_0000;
        let page = host_page_size().unwrap();
        let arena = RamArena::new(page as u64, page * 2).unwrap();
        let code = arena.resolve(page as u64, page).unwrap();
        let data = arena.resolve((page * 2) as u64, page).unwrap();
        // mov w0, #42; ret
        const SVC: u32 = 0xd400_2461;
        let instructions = [
            0x5280_0540,
            0xd65f_03c0,
            SVC,
            SVC,
            SYSCALL_BREAKPOINT,
            0xd51b_d05d,
        ];
        unsafe {
            ptr::copy_nonoverlapping(
                instructions.as_ptr(),
                code.alias.as_ptr().cast(),
                instructions.len(),
            );
            data.alias.as_ptr().write(7);
        }
        let text = unsafe {
            Mapping::install(
                IMAGE,
                page,
                code.backing,
                code.offset,
                Protection::ReadExecute(&[0..12, 16..24]),
            )
        }
        .unwrap();
        let data_page = unsafe {
            Mapping::install(
                IMAGE + page,
                page,
                data.backing,
                data.offset,
                Protection::ReadWrite,
            )
        }
        .unwrap();
        let collision = unsafe {
            Mapping::install(IMAGE, page, data.backing, data.offset, Protection::ReadOnly)
        };
        assert!(collision.is_err());
        unsafe {
            let call: extern "C" fn() -> u32 = std::mem::transmute(text.address());
            assert_eq!(call(), 42);
            let code_words = text.base.as_ptr().cast::<u32>();
            assert_eq!(code_words.add(2).read(), SYSCALL_BREAKPOINT);
            assert_eq!(code_words.add(3).read(), SVC);
            assert_eq!(code.alias.as_ptr().cast::<u32>().add(2).read(), SVC);
            let mapped = ptr::with_exposed_provenance_mut::<u8>(data_page.address());
            assert_eq!(mapped.read(), 7);
            mapped.write(9);
            assert_eq!(data.alias.as_ptr().read(), 9);
            #[cfg(target_os = "macos")]
            assert_ne!(
                libc::mprotect(text.base.as_ptr(), page, libc::PROT_WRITE),
                0
            );
        }
        assert_eq!(
            text.patched_instruction(IMAGE + 8, SYSCALL_BREAKPOINT),
            Some(PatchedInstruction::Syscall { immediate: 0x123 })
        );
        assert_eq!(text.published_word(IMAGE + 8), Some(SVC));
        assert_eq!(text.patched_instruction(IMAGE + 8, SVC), None);
        assert_eq!(
            text.patched_instruction(IMAGE + 16, SYSCALL_BREAKPOINT),
            None
        );
        assert_eq!(
            text.patched_instruction(IMAGE + 20, 0xd420_0000 | (0x5500 << 5)),
            Some(PatchedInstruction::WriteThreadPointer { register: 29 })
        );
    }
}
