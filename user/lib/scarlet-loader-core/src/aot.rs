//! Inspect self-contained AArch64 ELF images before packaging them for iOS.

use crate::{Error, Machine, elf::Parsed};
use alloc::{collections::BTreeSet, vec::Vec};
use core::ops::Range;

pub use crate::elf::Segment;

pub struct Layout {
    pub entry: usize,
    /// Address and entry count of a file-backed ELF program header table.
    pub program_headers: Option<(usize, usize)>,
    pub address_span: Range<usize>,
    pub segments: Vec<Segment>,
    pub tls: Option<(Segment, usize)>,
    pub relro: Option<Range<usize>>,
    /// Virtual address of each writable pointer and its referenced address.
    pub relative_pointers: Vec<(usize, usize)>,
}

pub fn inspect_aarch64_pie(bytes: &[u8], page: usize) -> Result<Layout, Error> {
    if page < crate::PAGE_SIZE || !page.is_power_of_two() {
        return Err(Error::Format("invalid AOT page size"));
    }
    let mut image = Parsed::parse_with_page_size(bytes, Machine::Aarch64, page)?;
    if image.executable || image.has_interpreter || !image.needed.is_empty() {
        return Err(Error::Unsupported(
            "requires a self-contained PIE without PT_INTERP",
        ));
    }
    if !image.try_native_pages(page) || image.align != page {
        return Err(Error::Unsupported(
            "PT_LOAD or RELRO is not aligned for the host",
        ));
    }
    let text = image
        .segments
        .iter()
        .filter(|segment| segment.permissions.execute);
    if text.count() != 1
        || image
            .segments
            .iter()
            .any(|segment| !segment.permissions.read)
    {
        return Err(Error::Unsupported(
            "requires one RX segment and readable load segments",
        ));
    }
    if image.init.is_some_and(|entry| entry != 0)
        || image.init_array.is_some_and(|(_, size)| size != 0)
    {
        return Err(Error::Unsupported(
            "guest initializer tables require a dynamic catalog",
        ));
    }
    if !image.segments.iter().any(|s| {
        s.permissions.execute
            && s.contains(image.entry, 4)
            && image.entry.is_multiple_of(4)
            && image.entry - s.vaddr + 4 <= s.file_size
    }) {
        return Err(Error::Format("entry is outside file-backed instructions"));
    }
    let mut targets = BTreeSet::new();
    let mut relative_pointers = Vec::new();
    for relocation in &image.relocations {
        if relocation.kind != 1027 {
            return Err(Error::Unsupported("only R_AARCH64_RELATIVE is supported"));
        }
        let value = usize::try_from(relocation.addend).map_err(|_| Error::Overflow)?;
        if !relocation.offset.is_multiple_of(8)
            || !targets.insert(relocation.offset)
            || !image
                .segments
                .iter()
                .any(|s| (s.vaddr..=s.vaddr + s.memory_size).contains(&value))
        {
            return Err(Error::Format(
                "invalid relative relocation target or addend",
            ));
        }
        relative_pointers.push((relocation.offset, value));
    }
    // The main ELF parser already checked the table's bounds and 56-byte entries.
    let first = u64::from_le_bytes(bytes[32..40].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(bytes[56..58].try_into().unwrap()) as usize;
    let end = first + 56 * count;
    let program_headers = image
        .segments
        .iter()
        .find(|segment| segment.offset <= first && end <= segment.offset + segment.file_size)
        .map(|segment| (segment.vaddr + (first - segment.offset), count));
    Ok(Layout {
        entry: image.entry,
        program_headers,
        address_span: image.min_vaddr..image.max_vaddr,
        segments: image.segments,
        tls: image.tls.map(|tls| (tls, image.tls_alignment)),
        relro: image.relro.map(|(begin, end)| begin..end),
        relative_pointers,
    })
}
