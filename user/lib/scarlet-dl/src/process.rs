use scarlet_loader_core::{Error, Permissions};
use std::ffi::{CStr, c_char};

const AT_EXECFD: usize = 2;
const AT_PHDR: usize = 3;
const AT_PHENT: usize = 4;
const AT_PHNUM: usize = 5;
const AT_PAGESZ: usize = 6;
const AT_ENTRY: usize = 9;
const AT_EXECFN: usize = 31;
pub(crate) const PAGE_SIZE: usize = 4096;

pub(crate) struct Process {
    pub path: String,
    pub bytes: Vec<u8>,
    pub bias: usize,
    pub entry: usize,
    pub segments: Vec<(usize, usize, Permissions)>,
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        bytes
            .get(at..at + 2)
            .ok_or(Error::Format("truncated main ELF"))?
            .try_into()
            .unwrap(),
    ))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at + 4)
            .ok_or(Error::Format("truncated main ELF"))?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<usize, Error> {
    usize::try_from(u64::from_le_bytes(
        bytes
            .get(at..at + 8)
            .ok_or(Error::Format("truncated main ELF"))?
            .try_into()
            .unwrap(),
    ))
    .map_err(|_| Error::Overflow)
}

struct MainLayout {
    bias: usize,
    segments: Vec<(usize, usize, Permissions)>,
    file_segments: Vec<(usize, usize, usize)>,
}

impl MainLayout {
    fn verify_file_segments(
        &self,
        bytes: &[u8],
        mut matches_mapping: impl FnMut(usize, &[u8]) -> bool,
    ) -> Result<(), Error> {
        for &(address, offset, size) in &self.file_segments {
            if !matches_mapping(address, &bytes[offset..offset + size]) {
                return Err(Error::Format(
                    "executable file differs from a kernel-mapped PT_LOAD",
                ));
            }
        }
        Ok(())
    }
}

/// Validate all adopted ranges before creating per-page metadata or reading
/// memory through addresses derived from the file.
fn main_layout(
    bytes: &[u8],
    phdr: usize,
    phent: usize,
    phnum: usize,
    entry: usize,
) -> Result<MainLayout, Error> {
    if bytes.get(..7) != Some(b"\x7fELF\x02\x01\x01") || phent != 56 || phnum == 0 || phnum > 1024 {
        return Err(Error::Format("invalid main ELF or program-header auxv"));
    }
    let kind = u16_at(bytes, 16)?;
    if kind != 2 && kind != 3 {
        return Err(Error::Unsupported("main must be ET_EXEC or ET_DYN"));
    }
    let file_phoff = u64_at(bytes, 32)?;
    if usize::from(u16_at(bytes, 54)?) != phent || usize::from(u16_at(bytes, 56)?) != phnum {
        return Err(Error::Format("main ELF and auxv headers disagree"));
    }
    let table_size = phent.checked_mul(phnum).ok_or(Error::Overflow)?;
    let table_end = file_phoff.checked_add(table_size).ok_or(Error::Overflow)?;
    if table_end > bytes.len() {
        return Err(Error::Format("truncated main program headers"));
    }
    let mut phdr_vaddr = None;
    let mut declared_phdr = None;
    let mut load_headers = Vec::new();
    let mut lowest = usize::MAX;
    let mut highest = 0;
    let mut metadata_span = 0usize;
    for i in 0..phnum {
        let at = file_phoff + i * phent;
        let segment_kind = u32_at(bytes, at)?;
        let offset = u64_at(bytes, at + 8)?;
        let vaddr = u64_at(bytes, at + 16)?;
        let filesz = u64_at(bytes, at + 32)?;
        let memsz = u64_at(bytes, at + 40)?;
        if segment_kind == 6 {
            if declared_phdr
                .replace((vaddr, offset, filesz, memsz))
                .is_some()
            {
                return Err(Error::Format("multiple PT_PHDR segments"));
            }
        }
        if segment_kind != 1 {
            continue;
        }
        let file_end = offset.checked_add(filesz).ok_or(Error::Overflow)?;
        if filesz > memsz || file_end > bytes.len() {
            return Err(Error::Format("invalid main PT_LOAD file range"));
        }
        let memory_end = vaddr.checked_add(memsz).ok_or(Error::Overflow)?;
        let flags = u32_at(bytes, at + 4)?;
        let permissions = Permissions {
            read: flags & 4 != 0,
            write: flags & 2 != 0,
            execute: flags & 1 != 0,
        };
        if filesz != 0 && !permissions.read {
            return Err(Error::Unsupported(
                "main file-backed PT_LOAD must be readable for adoption",
            ));
        }
        if file_phoff >= offset && table_end <= file_end {
            let candidate = vaddr
                .checked_add(file_phoff - offset)
                .ok_or(Error::Overflow)?;
            if phdr_vaddr.is_some_and(|previous| previous != candidate) {
                return Err(Error::Format("ambiguous loaded program-header location"));
            }
            phdr_vaddr.get_or_insert(candidate);
        }
        if memsz == 0 {
            continue;
        }
        let begin = vaddr & !(PAGE_SIZE - 1);
        let end = memory_end
            .checked_add(PAGE_SIZE - 1)
            .ok_or(Error::Overflow)?
            & !(PAGE_SIZE - 1);
        lowest = lowest.min(begin);
        highest = highest.max(end);
        metadata_span = metadata_span
            .checked_add(end - begin)
            .ok_or(Error::Overflow)?;
        if highest - lowest > scarlet_loader_core::MAX_IMAGE_SIZE
            || metadata_span > scarlet_loader_core::MAX_IMAGE_SIZE
        {
            return Err(Error::Unsupported("main image exceeds size limit"));
        }
        load_headers.push((vaddr, memsz, offset, filesz, permissions));
    }
    let phdr_vaddr = phdr_vaddr.ok_or(Error::Format("program headers are not loadable"))?;
    if let Some((vaddr, offset, filesz, memsz)) = declared_phdr {
        if vaddr != phdr_vaddr
            || offset != file_phoff
            || filesz != table_size
            || memsz != table_size
        {
            return Err(Error::Format(
                "PT_PHDR disagrees with its containing PT_LOAD",
            ));
        }
    }
    // The kernel's program_headers_address uses the PT_LOAD containing e_phoff.
    // PT_PHDR is only a consistency check and must never select the load bias.
    let bias = phdr.checked_sub(phdr_vaddr).ok_or(Error::Overflow)?;
    if kind == 2 && bias != 0 {
        return Err(Error::Format("ET_EXEC main has nonzero load bias"));
    }
    if bias
        .checked_add(u64_at(bytes, 24)?)
        .ok_or(Error::Overflow)?
        != entry
    {
        return Err(Error::Format("main ELF and AT_ENTRY disagree"));
    }
    let mut segments = Vec::new();
    let mut file_segments = Vec::new();
    for (vaddr, memsz, offset, filesz, permissions) in load_headers {
        let address = bias.checked_add(vaddr).ok_or(Error::Overflow)?;
        let memory_end = address.checked_add(memsz).ok_or(Error::Overflow)?;
        let begin = address & !(PAGE_SIZE - 1);
        let end = memory_end
            .checked_add(PAGE_SIZE - 1)
            .ok_or(Error::Overflow)?
            & !(PAGE_SIZE - 1);
        segments.push((begin, end - begin, permissions));
        if filesz != 0 {
            file_segments.push((address, offset, filesz));
        }
    }
    Ok(MainLayout {
        bias,
        segments,
        file_segments,
    })
}

#[derive(Default)]
struct AuxiliaryVector {
    phdr: usize,
    phent: usize,
    phnum: usize,
    pagesz: usize,
    entry: usize,
    execfn: Option<usize>,
    execfd: Option<usize>,
}

impl AuxiliaryVector {
    fn parse(pairs: impl IntoIterator<Item = (usize, usize)>) -> Result<Self, Error> {
        let mut aux = Self::default();
        let mut terminated = false;
        for (key, value) in pairs.into_iter().take(128) {
            match key {
                0 => {
                    terminated = true;
                    break;
                }
                AT_PHDR => aux.phdr = value,
                AT_PHENT => aux.phent = value,
                AT_PHNUM => aux.phnum = value,
                AT_PAGESZ => aux.pagesz = value,
                AT_ENTRY => aux.entry = value,
                AT_EXECFN => aux.execfn = Some(value),
                AT_EXECFD => aux.execfd = Some(value),
                _ => {}
            }
        }
        if !terminated
            || aux.phdr == 0
            || aux.entry == 0
            || aux.pagesz != PAGE_SIZE
            || aux.execfd.is_none()
        {
            return Err(Error::Format("missing or unsupported interpreter auxv"));
        }
        Ok(aux)
    }
}

/// Consume the native file handle installed by the kernel, including handle 0.
/// File owns the handle and closes it on success or any seek/read failure.
///
/// # Safety
/// The handle is the open, exclusively owned AT_EXECFD file handed to this
/// interpreter by the Scarlet kernel; no other Rust wrapper owns it.
unsafe fn read_executable_handle(handle: usize) -> Result<Vec<u8>, Error> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::fd::FromRawFd;
    let fd = i32::try_from(handle)
        .map_err(|_| Error::Format("AT_EXECFD exceeds native handle range"))?;
    // SAFETY: The caller transfers ownership of the kernel-provided handle.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.seek(SeekFrom::Start(0))
        .map_err(|e| Error::Platform(format!("seek AT_EXECFD: {e}")))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| Error::Platform(format!("read AT_EXECFD: {e}")))?;
    Ok(bytes)
}

impl Process {
    /// # Safety
    /// `stack` is the persistent initial stack supplied by the Scarlet kernel,
    /// including valid argv/envp/auxv arrays and their pointed-to strings.
    pub unsafe fn from_stack(stack: usize) -> Result<Self, Error> {
        if stack == 0 || stack % std::mem::align_of::<usize>() != 0 {
            return Err(Error::Format("invalid initial stack"));
        }
        let words = stack as *const usize;
        // SAFETY: The caller supplies the kernel initial stack.
        let argc = unsafe { *words };
        if argc > 65536 {
            return Err(Error::Format("excessive argc"));
        }
        let mut cursor = argc + 2;
        // SAFETY: argv has argc entries followed by its terminating null.
        if unsafe { *words.add(cursor - 1) } != 0 {
            return Err(Error::Format("unterminated argv"));
        }
        let mut env_count = 0;
        // SAFETY: envp follows argv on the kernel's initial stack.
        while unsafe { *words.add(cursor) } != 0 {
            cursor += 1;
            env_count += 1;
            if env_count > 65536 {
                return Err(Error::Format("excessive envp"));
            }
        }
        cursor += 1;
        let mut pairs = Vec::new();
        for _ in 0..128 {
            // SAFETY: auxv consists of native-word key/value pairs after envp.
            let key = unsafe { *words.add(cursor) };
            let value = unsafe { *words.add(cursor + 1) };
            cursor += 2;
            pairs.push((key, value));
            if key == 0 {
                break;
            }
        }
        let aux = AuxiliaryVector::parse(pairs)?;
        let (phdr, phent, phnum, entry) = (aux.phdr, aux.phent, aux.phnum, aux.entry);
        // Own and consume AT_EXECFD before fallible pathname processing. Its
        // underlying file can originate in a different filesystem view.
        let handle = aux
            .execfd
            .ok_or(Error::Format("missing authoritative AT_EXECFD"))?;
        // SAFETY: AuxiliaryVector requires the kernel-owned executable handle.
        let bytes = unsafe { read_executable_handle(handle) }?;
        let supplied_path = match aux.execfn.filter(|path| *path != 0) {
            Some(pointer) => {
                // SAFETY: The kernel supplies a persistent NUL-terminated
                // absolute pathname when it can verify one in this view.
                let path = unsafe { CStr::from_ptr(pointer as *const c_char) }
                    .to_str()
                    .map_err(|_| Error::Unsupported("non-UTF-8 executable pathname"))?;
                if !std::path::Path::new(path).has_root() {
                    return Err(Error::Format("AT_EXECFN must be absolute"));
                }
                Some(path.to_owned())
            }
            None => None,
        };
        // The optional path is only an origin/identity hint. The authoritative
        // executable is always read through AT_EXECFD, never reopened by name.
        let path = supplied_path.unwrap_or_else(|| format!("<main@{phdr:x}>"));
        let layout = main_layout(&bytes, phdr, phent, phnum, entry)?;
        let phoff = u64_at(&bytes, 32)?;
        let table_size = phent.checked_mul(phnum).ok_or(Error::Overflow)?;
        // SAFETY: Kernel auxv supplies this readable mapped program-header
        // table. main_layout validated the matching file table's full bounds.
        let mapped_headers = unsafe { std::slice::from_raw_parts(phdr as *const u8, table_size) };
        if mapped_headers != &bytes[phoff..phoff + table_size] {
            return Err(Error::Format(
                "executable file differs from the kernel-mapped headers",
            ));
        }
        // Headers now match the kernel's mapped table and the bias was derived
        // only from its containing PT_LOAD. Every range below is bounded by
        // that same table, and nonempty file-backed segments were required to
        // be readable. The kernel has not applied any dynamic relocations yet.
        layout.verify_file_segments(&bytes, |address, expected| {
            // SAFETY: The validated, byte-identical kernel program headers
            // establish this readable mapped range, as described above.
            unsafe { std::slice::from_raw_parts(address as *const u8, expected.len()) == expected }
        })?;
        Ok(Self {
            path,
            bytes,
            bias: layout.bias,
            entry,
            segments: layout.segments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf() -> Vec<u8> {
        let mut b = vec![0; 176];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        b[16..18].copy_from_slice(&3u16.to_le_bytes());
        b[24..32].copy_from_slice(&0x100u64.to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&2u16.to_le_bytes());
        b[64..68].copy_from_slice(&1u32.to_le_bytes());
        b[68..72].copy_from_slice(&4u32.to_le_bytes());
        b[80..88].copy_from_slice(&0x2000u64.to_le_bytes());
        b[96..104].copy_from_slice(&176u64.to_le_bytes());
        b[104..112].copy_from_slice(&5000u64.to_le_bytes());
        b
    }

    #[test]
    fn derives_bias_from_load_segment_without_pt_phdr() {
        let layout = main_layout(&elf(), 0x802040, 56, 2, 0x800100).unwrap();
        assert_eq!(layout.bias, 0x800000);
        assert_eq!(
            layout.segments,
            vec![(
                0x802000,
                8192,
                Permissions {
                    read: true,
                    write: false,
                    execute: false
                }
            )]
        );
    }

    #[test]
    fn rejects_spoofed_pt_phdr_before_its_containing_load() {
        let mut bytes = elf();
        let load = bytes[64..120].to_vec();
        bytes[120..176].copy_from_slice(&load);
        bytes[64..120].fill(0);
        bytes[64..68].copy_from_slice(&6u32.to_le_bytes());
        bytes[72..80].copy_from_slice(&64u64.to_le_bytes());
        bytes[80..88].copy_from_slice(&0x2040u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&112u64.to_le_bytes());
        bytes[104..112].copy_from_slice(&112u64.to_le_bytes());
        assert_eq!(
            main_layout(&bytes, 0x802040, 56, 2, 0x800100).unwrap().bias,
            0x800000
        );
        bytes[80..88].copy_from_slice(&0x1040u64.to_le_bytes());
        // The old code accepted a matching AT_ENTRY for the spoofed bias.
        assert!(matches!(
            main_layout(&bytes, 0x802040, 56, 2, 0x801100),
            Err(Error::Format(
                "PT_PHDR disagrees with its containing PT_LOAD"
            ))
        ));
    }

    #[test]
    fn bounds_image_span_and_page_metadata_before_allocation() {
        let mut bytes = elf();
        let too_large = (scarlet_loader_core::MAX_IMAGE_SIZE + PAGE_SIZE) as u64;
        bytes[104..112].copy_from_slice(&too_large.to_le_bytes());
        assert!(matches!(
            main_layout(&bytes, 0x802040, 56, 2, 0x800100),
            Err(Error::Unsupported("main image exceeds size limit"))
        ));
        // Even overlapping ranges cannot multiply per-page metadata past the
        // budget while keeping the virtual span below the image limit.
        let half_plus_page = (scarlet_loader_core::MAX_IMAGE_SIZE / 2 + PAGE_SIZE) as u64;
        bytes[104..112].copy_from_slice(&half_plus_page.to_le_bytes());
        let duplicate = bytes[64..120].to_vec();
        bytes[120..176].copy_from_slice(&duplicate);
        assert!(matches!(
            main_layout(&bytes, 0x802040, 56, 2, 0x800100),
            Err(Error::Unsupported("main image exceeds size limit"))
        ));
    }

    #[test]
    fn rejects_unreadable_or_out_of_file_load_ranges() {
        let mut bytes = elf();
        bytes[68..72].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            main_layout(&bytes, 0x802040, 56, 2, 0x800100),
            Err(Error::Unsupported(
                "main file-backed PT_LOAD must be readable for adoption"
            ))
        ));
        let mut bytes = elf();
        bytes[96..104].copy_from_slice(&177u64.to_le_bytes());
        assert!(matches!(
            main_layout(&bytes, 0x802040, 56, 2, 0x800100),
            Err(Error::Format("invalid main PT_LOAD file range"))
        ));
        let mut bytes = elf();
        bytes[104..112].copy_from_slice(&16u64.to_le_bytes());
        assert!(matches!(
            main_layout(&bytes, 0x802040, 56, 2, 0x800100),
            Err(Error::Format("invalid main PT_LOAD file range"))
        ));
    }

    #[test]
    fn compares_load_contents_even_when_program_headers_match() {
        let mut bytes = elf();
        bytes.extend_from_slice(b"original payload");
        let size = bytes.len() as u64;
        bytes[96..104].copy_from_slice(&size.to_le_bytes());
        let layout = main_layout(&bytes, 0x802040, 56, 2, 0x800100).unwrap();
        let mut mapped = bytes.clone();
        assert!(
            layout
                .verify_file_segments(&bytes, |address, expected| {
                    let offset = address - 0x802000;
                    expected == &mapped[offset..offset + expected.len()]
                })
                .is_ok()
        );
        mapped[176] ^= 1;
        assert_eq!(&mapped[64..176], &bytes[64..176]);
        assert!(
            layout
                .verify_file_segments(&bytes, |address, expected| {
                    let offset = address - 0x802000;
                    expected == &mapped[offset..offset + expected.len()]
                })
                .is_err()
        );
    }

    #[test]
    fn execfd_presence_accepts_handle_zero_without_execfn() {
        let base = [
            (AT_PHDR, 0x802040),
            (AT_PHENT, 56),
            (AT_PHNUM, 2),
            (AT_PAGESZ, PAGE_SIZE),
            (AT_ENTRY, 0x800100),
        ];
        let with_fd = base.into_iter().chain([(AT_EXECFD, 0), (0, 0)]);
        let aux = AuxiliaryVector::parse(with_fd).unwrap();
        assert_eq!(aux.execfd, Some(0));
        assert_eq!(aux.execfn, None);
        assert!(AuxiliaryVector::parse(base.into_iter().chain([(0, 0)])).is_err());
        assert!(
            AuxiliaryVector::parse(base.into_iter().chain([(AT_EXECFN, 0x9000), (0, 0)])).is_err()
        );
        assert!(AuxiliaryVector::parse(base.into_iter().chain([(AT_EXECFN, 0), (0, 0)])).is_err());
        assert!(AuxiliaryVector::parse(base.into_iter().chain([(AT_EXECFD, 4)])).is_err());
    }

    #[test]
    fn rejects_auxv_file_disagreement_and_truncation() {
        assert!(main_layout(&elf(), 0x802040, 56, 1, 0x800100).is_err());
        assert!(main_layout(&elf(), 0x802040, 56, 2, 0x800104).is_err());
        assert!(main_layout(&elf()[..170], 0x802040, 56, 2, 0x800100).is_err());
    }
}
