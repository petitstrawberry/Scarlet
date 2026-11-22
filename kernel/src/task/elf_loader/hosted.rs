//! Final image protections for the hosted executable mappings.

use super::{
    ET_EXEC, ElfHeader, ElfLoaderError, FileObject, PAGE_SIZE, PF_R, PF_X, PT_INTERP, PT_LOAD,
    Task, VirtualMemoryPermission, elf_error, for_each_program_header,
};
use crate::vm::vmem::MemoryArea;

const PT_DYNAMIC: u32 = 2;
const PT_GNU_RELRO: u32 = 0x6474_e552;

/// Choose native ELF placement only when each native protection page has a single owner and
/// sealing RELRO cannot affect any required writable bytes.
pub(super) fn use_native_load_pages(
    header: &ElfHeader,
    file: &dyn FileObject,
    bias: u64,
) -> Result<bool, ElfLoaderError> {
    if header.ei_osabi != super::ELFOSABI_SCARLET {
        return Ok(false);
    }
    let page = crate::vm::execution_page_size() as u64;
    let floor = |v: u64| v & !(page - 1);
    let mut loads = alloc::vec::Vec::new();
    let mut relro = None;
    let mut eligible = bias.is_multiple_of(page);
    for_each_program_header(header, file, |_, ph| {
        if ph.p_memsz == 0 || !matches!(ph.p_type, PT_LOAD | PT_GNU_RELRO) {
            return Ok(true);
        }
        let end = ph
            .p_vaddr
            .checked_add(ph.p_memsz)
            .ok_or_else(|| elf_error("ELF segment end overflow"))?;
        let span = ph.p_vaddr..end;
        if ph.p_type == PT_GNU_RELRO {
            eligible &= relro.replace(span).is_none();
        } else {
            eligible &= ph.p_align >= page && ph.p_offset % page == ph.p_vaddr % page;
            loads.push((span, ph.p_flags));
        }
        Ok(eligible)
    })?;
    loads.sort_unstable_by_key(|(span, _)| span.start);
    if !eligible
        || loads.is_empty()
        || loads
            .windows(2)
            .any(|pair| floor(pair[0].0.end - 1) >= floor(pair[1].0.start))
    {
        return Ok(false);
    }
    if let Some(relro) = relro {
        for (span, flags) in &loads {
            if flags & super::PF_W == 0 {
                continue;
            }
            let intersects = floor(span.start) <= floor(relro.end - 1)
                && floor(relro.start) <= floor(span.end - 1);
            if intersects
                && ((span.start < relro.start && !relro.start.is_multiple_of(page))
                    || (span.end > relro.end && !relro.end.is_multiple_of(page)))
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// A fixed static executable has no runtime relocator to seal its RELRO.
/// In particular, scarlet-ld only seals the application and its libraries.
pub(super) fn seal_static_relro(
    header: &ElfHeader,
    file: &dyn FileObject,
    task: &Task,
) -> Result<(), ElfLoaderError> {
    if header.e_type != ET_EXEC {
        return Ok(());
    }
    let mut dynamic = false;
    let mut relro = None;
    for_each_program_header(header, file, |_, ph| {
        match ph.p_type {
            PT_DYNAMIC | PT_INTERP => dynamic = true,
            PT_GNU_RELRO if ph.p_memsz != 0 => {
                let end = ph
                    .p_vaddr
                    .checked_add(ph.p_memsz)
                    .ok_or_else(|| elf_error("ELF RELRO overflows"))?;
                if relro.replace(ph.p_vaddr..end).is_some() {
                    return Err(elf_error("Duplicate ELF RELRO segment"));
                }
            }
            _ => {}
        }
        Ok(true)
    })?;
    let Some(relro) = relro.filter(|_| !dynamic) else {
        return Ok(());
    };
    let start = relro.start & !(PAGE_SIZE as u64 - 1);
    let end = relro.end & !(PAGE_SIZE as u64 - 1);
    if start == end {
        return Ok(());
    }
    let mut permissions = None;
    for_each_program_header(header, file, |_, ph| {
        if ph.p_type != PT_LOAD {
            return Ok(true);
        }
        let load_end = ph.p_vaddr + ph.p_memsz;
        if ph.p_vaddr > relro.start || load_end < relro.end {
            return Ok(true);
        }
        let mut flags = 0;
        if ph.p_flags & PF_R != 0 {
            flags |= VirtualMemoryPermission::Read as usize;
        }
        if ph.p_flags & PF_X != 0 {
            flags |= VirtualMemoryPermission::Execute as usize;
        }
        permissions = Some(flags);
        Ok(false)
    })?;
    let permissions = permissions.ok_or_else(|| elf_error("ELF RELRO is outside PT_LOAD"))?;
    let area = MemoryArea::new(start as usize, (end - 1) as usize);
    task.vm_manager
        .protect_memory_map_range(area, permissions)
        .map_err(elf_error)
}

/// AArch64 ELF marks inline data with $d and the next instruction with $x.
/// Keep the declarations tied to the mapping origin across protection splits.
pub(super) fn record_instructions(
    header: &ElfHeader,
    file: &dyn FileObject,
    task: &Task,
    bias: u64,
) -> Result<(), ElfLoaderError> {
    visit_instructions(header, file, |address, _, size| {
        let lo = bias
            .checked_add(address)
            .ok_or_else(|| elf_error("ELF text bias overflow"))? as usize;
        let hi = lo
            .checked_add(size)
            .ok_or_else(|| elf_error("ELF text end overflow"))?;
        let map = task
            .vm_manager
            .search_memory_map(lo)
            .ok_or_else(|| elf_error("ELF text outside mapping"))?;
        task.vm_manager
            .hosted_image()
            .add_code(map.mapping_origin, lo..hi);
        Ok(())
    })
}

pub(crate) fn visit_instructions(
    header: &ElfHeader,
    file: &dyn FileObject,
    mut visit: impl FnMut(u64, u64, usize) -> Result<(), ElfLoaderError>,
) -> Result<(), ElfLoaderError> {
    use crate::lsm::elf::{parse_shdr, parse_symtab_entry};
    use alloc::{collections::BTreeMap, vec::Vec};
    let file_size = file
        .metadata()
        .map_err(|_| elf_error("Cannot size ELF"))?
        .size as u64;
    let bytes = |offset: u64, size: u64| -> Result<Vec<u8>, ElfLoaderError> {
        if offset.checked_add(size).is_none_or(|end| end > file_size) {
            return Err(elf_error("ELF section outside file"));
        }
        let mut data = Vec::new();
        data.try_reserve_exact(size as usize)
            .map_err(|_| elf_error("Cannot allocate ELF metadata"))?;
        data.resize(size as usize, 0);
        let mut read = 0;
        while read < data.len() {
            let n = file
                .read_at(offset + read as u64, &mut data[read..])
                .map_err(|_| elf_error("Cannot read ELF metadata"))?;
            if n == 0 || n > data.len() - read {
                return Err(elf_error("Truncated ELF metadata"));
            }
            read += n;
        }
        Ok(data)
    };
    if header.e_shnum == 0 {
        return Ok(());
    }
    let stride = usize::from(header.e_shentsize);
    if stride < 64 {
        return Err(elf_error("Invalid ELF section stride"));
    }
    let table = bytes(
        header.e_shoff,
        (stride * usize::from(header.e_shnum)) as u64,
    )?;
    let sections = table
        .chunks_exact(stride)
        .map(|entry| parse_shdr(entry, 0, true).map_err(elf_error))
        .collect::<Result<Vec<_>, _>>()?;
    let mut markers = BTreeMap::new();
    for section in sections.iter().filter(|s| s.sh_type == 2) {
        let strings = sections
            .get(section.sh_link as usize)
            .ok_or_else(|| elf_error("Invalid ELF string table"))?;
        let names = bytes(strings.sh_offset, strings.sh_size)?;
        let symbols = bytes(section.sh_offset, section.sh_size)?;
        if section.sh_entsize != 24 || !symbols.len().is_multiple_of(24) {
            return Err(elf_error("Invalid ELF symbol table"));
        }
        for entry in symbols.chunks_exact(24) {
            let symbol = parse_symtab_entry(entry, 0, true).map_err(elf_error)?;
            let Some(name) = names.get(symbol.st_name as usize..) else {
                continue;
            };
            if name.first() == Some(&b'$')
                && matches!(name.get(2), Some(0 | b'.'))
                && matches!(name.get(1), Some(b'd' | b'x'))
            {
                markers.insert((symbol.st_shndx as usize, symbol.st_value), name[1] == b'x');
            }
        }
    }
    for (index, section) in sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.sh_type == 1 && s.sh_flags & 6 == 6 && s.sh_size != 0)
    {
        let end = section
            .sh_addr
            .checked_add(section.sh_size)
            .ok_or_else(|| elf_error("ELF text overflows"))?;
        let mut code = true;
        let mut start = section.sh_addr;
        for (&(_, at), &next) in markers
            .range((index, start)..(index, end))
            .chain(core::iter::once((&(index, end), &false)))
        {
            if code && start < at {
                let offset = section
                    .sh_offset
                    .checked_add(start - section.sh_addr)
                    .ok_or_else(|| elf_error("ELF instruction offset overflow"))?;
                visit(start, offset, (at - start) as usize)?;
            }
            start = at;
            code = next;
        }
    }
    Ok(())
}
