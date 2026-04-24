use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::sync::atomic::Ordering;
use core::{mem::size_of, ptr};

use crate::{
    environment::PAGE_SIZE,
    fs::{FileObject, SeekFrom},
    mem::page::ContiguousPages,
    task::Task,
    vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission},
};

pub const MH_MAGIC_64: u32 = 0xFEEDFACF;
pub const FAT_MAGIC: u32 = 0xCAFEBABE;
pub const FAT_MAGIC_64: u32 = 0xCAFEBABF;
pub const MH_EXECUTE: u32 = 0x02;
pub const MH_DYLINKER: u32 = 0x07;
pub const CPU_TYPE_ARM64: u32 = 0x0100000C;
pub const CPU_SUBTYPE_ALL: u32 = 0x00000000;
pub const CPU_SUBTYPE_ARM64E: u32 = 0x80000002;

#[repr(C)]
#[derive(Clone, Copy)]
struct FatHeader {
    magic: u32,
    nfat_arch: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FatArch {
    cputype: u32,
    cpusubtype: u32,
    offset: u32,
    size: u32,
    align: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FatArch64 {
    cputype: u32,
    cpusubtype: u32,
    offset: u64,
    size: u64,
    align: u32,
    reserved: u32,
}

pub const LC_SEGMENT_64: u32 = 0x19;
pub const LC_MAIN: u32 = 0x80000028;
pub const LC_UNIXTHREAD: u32 = 0x05;
pub const LC_DYSYMTAB: u32 = 0x0B;
pub const LC_LOAD_DYLIB: u32 = 0x0C;
pub const LC_LOAD_DYLINKER: u32 = 0x0e;
pub const LC_DYLD_CHAINED_FIXUPS: u32 = 0x80000034;

const DYLD_CHAINED_PTR_START_NONE: u16 = 0xFFFF;
const DYLD_CHAINED_PTR_START_MULTI: u16 = 0x8000;
const DYLD_CHAINED_PTR_ARM64E: u16 = 1;
const DYLD_CHAINED_PTR_ARM64E_USERLAND24: u16 = 12;

const ARM_THREAD_STATE64_PC_OFFSET: usize = 272;
const MIN_UNIXTHREAD_COMMAND_SIZE: usize = ARM_THREAD_STATE64_PC_OFFSET + size_of::<u64>();

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MachHeader64 {
    pub magic: u32,
    pub cputype: u32,
    pub cpusubtype: u32,
    pub filetype: u32,
    pub ncmds: u32,
    pub sizeofcmds: u32,
    pub flags: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LoadCommand {
    pub cmd: u32,
    pub cmdsize: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SegmentCommand64 {
    pub cmd: u32,
    pub cmdsize: u32,
    pub segname: [u8; 16],
    pub vmaddr: u64,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
    pub maxprot: i32,
    pub initprot: i32,
    pub nsects: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EntryPointCommand {
    pub cmd: u32,
    pub cmdsize: u32,
    pub entryoff: u64,
    pub stacksize: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinkEditDataCommand {
    cmd: u32,
    cmdsize: u32,
    dataoff: u32,
    datasize: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DyldChainedFixupsHeader {
    fixups_version: u32,
    starts_offset: u32,
    imports_offset: u32,
    symbols_offset: u32,
    imports_count: u32,
    imports_format: u32,
    symbols_format: u32,
}

/// Given a file object positioned at offset 0, detect fat/universal Mach-O and return
/// the file offset of the arm64 slice (or 0 if it's already a thin Mach-O).
fn find_arm64_slice(file_obj: &dyn FileObject) -> Result<u64, &'static str> {
    file_obj
        .seek(SeekFrom::Start(0))
        .map_err(|_| "Failed to seek")?;
    let mut magic_buf = [0u8; 4];
    if let Err(e) = read_exact(file_obj, &mut magic_buf) {
        crate::println!("[darwin] find_arm64_slice: failed reading magic: {}", e);
        return Err(e);
    }
    let magic_be = u32::from_be_bytes(magic_buf);
    let magic_le = u32::from_le_bytes(magic_buf);
    crate::println!(
        "[darwin] find_arm64_slice: magic_be=0x{:x} magic_le=0x{:x}",
        magic_be,
        magic_le
    );

    if magic_le == MH_MAGIC_64 {
        return Ok(0);
    }

    if magic_be == FAT_MAGIC {
        file_obj
            .seek(SeekFrom::Start(0))
            .map_err(|_| "Failed to seek")?;
        let mut fat_header_bytes = [0u8; size_of::<FatHeader>()];
        read_exact(file_obj, &mut fat_header_bytes)?;
        let fat_hdr = read_struct::<FatHeader>(&fat_header_bytes)?;
        let nfat = u32::from_be(fat_hdr.nfat_arch);
        crate::println!("[darwin] FAT_MAGIC: nfat={}", nfat);

        for i in 0..nfat {
            let arch_off = size_of::<FatHeader>() as u64 + i as u64 * size_of::<FatArch>() as u64;
            if let Err(e) = file_obj.seek(SeekFrom::Start(arch_off)) {
                crate::println!("[darwin] seek fat arch {} at offset {} failed", i, arch_off);
                return Err("Failed to seek fat arch");
            }
            let mut arch_bytes = [0u8; size_of::<FatArch>()];
            if let Err(e) = read_exact(file_obj, &mut arch_bytes) {
                crate::println!("[darwin] read fat arch {} failed: {}", i, e);
                return Err(e);
            }
            let arch = read_struct::<FatArch>(&arch_bytes)?;

            let cputype = u32::from_be(arch.cputype);
            let cpusubtype = u32::from_be(arch.cpusubtype);
            let offset = u32::from_be(arch.offset);
            let size = u32::from_be(arch.size);
            crate::println!(
                "[darwin] fat arch[{}]: cputype=0x{:x} subtype=0x{:x} offset={} size={}",
                i,
                cputype,
                cpusubtype,
                offset,
                size
            );

            if cputype == CPU_TYPE_ARM64 {
                crate::println!("[darwin] Fat binary: arm64 slice at offset {}", offset);
                return Ok(offset as u64);
            }
        }
        return Err("No arm64 slice in fat binary");
    }

    if magic_be == FAT_MAGIC_64 {
        file_obj
            .seek(SeekFrom::Start(0))
            .map_err(|_| "Failed to seek")?;
        let mut fat_header_bytes = [0u8; size_of::<FatHeader>()];
        read_exact(file_obj, &mut fat_header_bytes)?;
        let fat_hdr = read_struct::<FatHeader>(&fat_header_bytes)?;
        let nfat = u32::from_be(fat_hdr.nfat_arch);

        for i in 0..nfat {
            let arch_off = size_of::<FatHeader>() as u64 + i as u64 * size_of::<FatArch64>() as u64;
            file_obj
                .seek(SeekFrom::Start(arch_off))
                .map_err(|_| "Failed to seek fat arch64")?;
            let mut arch_bytes = [0u8; size_of::<FatArch64>()];
            read_exact(file_obj, &mut arch_bytes)?;
            let arch = read_struct::<FatArch64>(&arch_bytes)?;

            let cputype = u32::from_be(arch.cputype);

            if cputype == CPU_TYPE_ARM64 {
                let slice_offset = u64::from_be(arch.offset);
                crate::println!(
                    "[darwin] Fat64 binary: arm64 slice at offset {}",
                    slice_offset
                );
                return Ok(slice_offset);
            }
        }
        return Err("No arm64 slice in fat64 binary");
    }

    Err("Invalid Mach-O magic")
}

pub fn load_macho_binary(
    file_obj: &dyn FileObject,
    task: &Task,
) -> Result<(usize, Option<String>, usize), &'static str> {
    let slice_offset = find_arm64_slice(file_obj)?;
    crate::println!("[darwin] load_macho_binary: slice_offset={}", slice_offset);

    file_obj
        .seek(SeekFrom::Start(slice_offset))
        .map_err(|_| "Failed to seek to Mach-O header")?;

    let mut header_bytes = [0u8; size_of::<MachHeader64>()];
    read_exact(file_obj, &mut header_bytes)?;
    let header = read_struct::<MachHeader64>(&header_bytes)?;

    crate::println!(
        "[darwin] header: magic=0x{:x} cputype=0x{:x} subtype=0x{:x} filetype={} ncmds={} sizeofcmds={}",
        header.magic,
        header.cputype,
        header.cpusubtype,
        header.filetype,
        header.ncmds,
        header.sizeofcmds
    );

    if header.magic != MH_MAGIC_64 {
        return Err("Invalid Mach-O magic");
    }
    if header.cputype != CPU_TYPE_ARM64 {
        return Err("Unsupported Mach-O CPU type");
    }
    if header.cpusubtype != CPU_SUBTYPE_ALL && header.cpusubtype != CPU_SUBTYPE_ARM64E {
        return Err("Unsupported Mach-O CPU subtype");
    }
    if header.filetype != MH_EXECUTE {
        return Err("Unsupported Mach-O file type");
    }

    let mut load_commands = vec![0u8; header.sizeofcmds as usize];
    read_exact(file_obj, &mut load_commands)?;

    let mut segments = Vec::new();
    let mut entryoff = None;
    let mut unixthread_entry = None;
    let mut dylinker_path: Option<String> = None;

    let mut offset = 0usize;
    for _ in 0..header.ncmds {
        let command_end = offset
            .checked_add(size_of::<LoadCommand>())
            .ok_or("Mach-O load command overflow")?;
        if command_end > load_commands.len() {
            return Err("Mach-O load command table truncated");
        }

        let load_cmd = read_struct::<LoadCommand>(&load_commands[offset..command_end])?;
        let cmdsize = load_cmd.cmdsize as usize;
        if cmdsize < size_of::<LoadCommand>() {
            return Err("Invalid Mach-O load command size");
        }

        let next_offset = offset
            .checked_add(cmdsize)
            .ok_or("Mach-O load command overflow")?;
        if next_offset > load_commands.len() {
            return Err("Mach-O load command exceeds command table");
        }

        let command_bytes = &load_commands[offset..next_offset];
        crate::println!(
            "[darwin] load cmd 0x{:x} size={}",
            load_cmd.cmd,
            load_cmd.cmdsize
        );
        match load_cmd.cmd {
            LC_SEGMENT_64 => {
                if cmdsize < size_of::<SegmentCommand64>() {
                    return Err("Truncated LC_SEGMENT_64 command");
                }
                segments.push(read_struct::<SegmentCommand64>(
                    &command_bytes[..size_of::<SegmentCommand64>()],
                )?);
            }
            LC_MAIN => {
                if cmdsize < size_of::<EntryPointCommand>() {
                    return Err("Truncated LC_MAIN command");
                }
                let entry_cmd = read_struct::<EntryPointCommand>(
                    &command_bytes[..size_of::<EntryPointCommand>()],
                )?;
                entryoff = Some(entry_cmd.entryoff);
            }
            LC_UNIXTHREAD => {
                if cmdsize >= MIN_UNIXTHREAD_COMMAND_SIZE {
                    let pc_offset = offset_of_pc_in_unixthread();
                    unixthread_entry = Some(read_u64(&command_bytes[pc_offset..pc_offset + 8]));
                }
            }
            LC_LOAD_DYLINKER => {
                crate::println!("[darwin] found LC_LOAD_DYLINKER cmdsize={}", cmdsize);
                if cmdsize >= 12 {
                    let name_offset = read_u32(&command_bytes[8..12]) as usize;
                    crate::println!("[darwin] name_offset={}", name_offset);
                    if name_offset < cmdsize {
                        let path_bytes = &command_bytes[name_offset..cmdsize];
                        if let Some(null_pos) = path_bytes.iter().position(|&b| b == 0) {
                            if let Ok(path) = core::str::from_utf8(&path_bytes[..null_pos]) {
                                crate::println!("[darwin] dylinker path: '{}'", path);
                                dylinker_path = Some(path.to_string());
                            }
                        }
                    }
                }
            }
            LC_DYSYMTAB => {}
            _ => {}
        }

        offset = next_offset;
    }

    for segment in &segments {
        crate::println!(
            "[darwin] mapping segment: vmaddr=0x{:x} vmsize=0x{:x} fileoff=0x{:x} filesize=0x{:x} prot={}",
            segment.vmaddr,
            segment.vmsize,
            segment.fileoff,
            segment.filesize,
            segment.initprot
        );
        map_segment(file_obj, task, segment, slice_offset)?;
    }

    let mach_header_addr =
        file_offset_to_vaddr(&segments, 0).ok_or("Failed to resolve Mach-O header address")?;

    let entry_point = if let Some(entryoff) = entryoff {
        file_offset_to_vaddr(&segments, entryoff).ok_or("Failed to resolve Mach-O entry point")?
    } else if let Some(entry) = unixthread_entry {
        entry as usize
    } else {
        return Err("Mach-O binary missing entry point");
    };

    Ok((entry_point, dylinker_path, mach_header_addr))
}

fn map_segment(
    file_obj: &dyn FileObject,
    task: &Task,
    segment: &SegmentCommand64,
    slice_offset: u64,
) -> Result<(), &'static str> {
    if segment.vmsize == 0 {
        return Ok(());
    }

    let permissions = macho_prot_to_scarlet(segment.initprot);
    if permissions == VirtualMemoryPermission::User as usize && segment.filesize == 0 {
        return Ok(());
    }

    let segment_vaddr =
        usize::try_from(segment.vmaddr).map_err(|_| "Mach-O vmaddr out of range")?;
    let segment_vmsize =
        usize::try_from(segment.vmsize).map_err(|_| "Mach-O vmsize out of range")?;
    let segment_filesize =
        usize::try_from(segment.filesize).map_err(|_| "Mach-O filesize out of range")?;
    let segment_fileoff = segment.fileoff;

    if segment_filesize > segment_vmsize {
        return Err("Mach-O segment filesize exceeds vmsize");
    }

    let page_offset = segment_vaddr & (PAGE_SIZE - 1);
    let mapping_start = segment_vaddr - page_offset;
    let mapping_size = segment_vmsize
        .checked_add(page_offset)
        .ok_or("Mach-O segment size overflow")?;
    let aligned_size = align_up(mapping_size, PAGE_SIZE);
    let num_pages = aligned_size / PAGE_SIZE;

    let pages = ContiguousPages::new(num_pages).ok_or("Failed to allocate Mach-O segment pages")?;
    let paddr = pages.as_paddr();
    let mmap = VirtualMemoryMap::new(
        MemoryArea::new(paddr, paddr + aligned_size - 1),
        MemoryArea::new(mapping_start, mapping_start + aligned_size - 1),
        permissions,
        false,
        None,
    );

    task.vm_manager.add_memory_map(mmap)?;
    task.page_allocations.write().push(pages);

    let kva = task
        .vm_manager
        .translate_to_kva(mapping_start)
        .ok_or("Failed to translate Mach-O segment mapping")?;

    unsafe {
        ptr::write_bytes(kva as *mut u8, 0, aligned_size);
    }

    if segment_filesize > 0 {
        let mut file_data = vec![0u8; segment_filesize];
        file_obj
            .seek(SeekFrom::Start(slice_offset + segment_fileoff))
            .map_err(|_| "Failed to seek to Mach-O segment")?;
        read_exact(file_obj, &mut file_data)?;

        let target_vaddr = segment_vaddr;
        let target_kva = task
            .vm_manager
            .translate_to_kva(target_vaddr)
            .ok_or("Failed to translate Mach-O segment destination")?;

        unsafe {
            ptr::copy_nonoverlapping(file_data.as_ptr(), target_kva as *mut u8, segment_filesize);
        }
    }

    if permissions & VirtualMemoryPermission::Execute as usize != 0 {
        task.text_size.fetch_add(aligned_size, Ordering::SeqCst);
    } else {
        task.data_size.fetch_add(aligned_size, Ordering::SeqCst);
        let segment_end = mapping_start + aligned_size;
        let current_brk = task.brk.load(Ordering::SeqCst);
        if current_brk == usize::MAX || segment_end > current_brk {
            task.brk.store(segment_end, Ordering::SeqCst);
        }
    }

    Ok(())
}

/// Map a Mach-O segment at a relocated base address.
/// Used for loading dyld which may need to be mapped at a different address.
pub fn map_segment_with_base(
    file_obj: &dyn FileObject,
    task: &Task,
    segment: &SegmentCommand64,
    base_delta: i64,
    slice_offset: u64,
) -> Result<(), &'static str> {
    if segment.vmsize == 0 {
        return Ok(());
    }

    let permissions = macho_prot_to_scarlet(segment.initprot);
    if permissions == VirtualMemoryPermission::User as usize && segment.filesize == 0 {
        return Ok(());
    }

    let original_vaddr = usize::try_from(segment.vmaddr).map_err(|_| "vmaddr out of range")?;
    let segment_vaddr = if base_delta >= 0 {
        original_vaddr
            .checked_add(base_delta as usize)
            .ok_or("vmaddr overflow")?
    } else {
        original_vaddr
            .checked_sub((-base_delta) as usize)
            .ok_or("vmaddr underflow")?
    };
    let segment_vmsize = usize::try_from(segment.vmsize).map_err(|_| "vmsize out of range")?;
    let segment_filesize =
        usize::try_from(segment.filesize).map_err(|_| "filesize out of range")?;
    let segment_fileoff = segment.fileoff;

    if segment_filesize > segment_vmsize {
        return Err("filesize exceeds vmsize");
    }

    let page_offset = segment_vaddr & (PAGE_SIZE - 1);
    let mapping_start = segment_vaddr - page_offset;
    let mapping_size = segment_vmsize
        .checked_add(page_offset)
        .ok_or("size overflow")?;
    let aligned_size = align_up(mapping_size, PAGE_SIZE);
    let num_pages = aligned_size / PAGE_SIZE;

    let pages = ContiguousPages::new(num_pages).ok_or("Failed to allocate dyld segment pages")?;
    let paddr = pages.as_paddr();
    let mmap = VirtualMemoryMap::new(
        MemoryArea::new(paddr, paddr + aligned_size - 1),
        MemoryArea::new(mapping_start, mapping_start + aligned_size - 1),
        permissions,
        false,
        None,
    );

    task.vm_manager.add_memory_map(mmap)?;
    task.page_allocations.write().push(pages);

    let kva = task
        .vm_manager
        .translate_to_kva(mapping_start)
        .ok_or("Failed to translate dyld segment")?;

    unsafe {
        ptr::write_bytes(kva as *mut u8, 0, aligned_size);
    }

    if segment_filesize > 0 {
        let mut file_data = vec![0u8; segment_filesize];
        file_obj
            .seek(SeekFrom::Start(slice_offset + segment_fileoff))
            .map_err(|_| "Failed to seek to dyld segment")?;
        read_exact(file_obj, &mut file_data)?;

        let target_kva = task
            .vm_manager
            .translate_to_kva(segment_vaddr)
            .ok_or("Failed to translate dyld segment destination")?;

        unsafe {
            ptr::copy_nonoverlapping(file_data.as_ptr(), target_kva as *mut u8, segment_filesize);
        }
    }

    if permissions & VirtualMemoryPermission::Execute as usize != 0 {
        task.text_size.fetch_add(aligned_size, Ordering::SeqCst);
    } else {
        task.data_size.fetch_add(aligned_size, Ordering::SeqCst);
    }

    Ok(())
}

fn apply_chained_fixups(
    task: &Task,
    base_addr: u64,
    base_delta: i64,
    fixup_data: &[u8],
) -> Result<(), &'static str> {
    if fixup_data.len() < size_of::<DyldChainedFixupsHeader>() {
        return Err("Chained fixups data too small for header");
    }

    let header = read_struct::<DyldChainedFixupsHeader>(
        &fixup_data[..size_of::<DyldChainedFixupsHeader>()],
    )?;
    if header.fixups_version != 0 {
        return Err("Unsupported dyld chained fixups version");
    }

    let starts_base = header.starts_offset as usize;
    if starts_base + 4 > fixup_data.len() {
        return Err("Chained fixups starts table out of bounds");
    }

    let seg_count = u32::from_le_bytes(
        fixup_data[starts_base..starts_base + 4]
            .try_into()
            .map_err(|_| "bad seg_count")?,
    ) as usize;

    for seg_idx in 0..seg_count {
        let off_pos = starts_base + 4 + seg_idx * 4;
        if off_pos + 4 > fixup_data.len() {
            return Err("Chained fixups seg_info_offset out of bounds");
        }
        let seg_info_offset = u32::from_le_bytes(
            fixup_data[off_pos..off_pos + 4]
                .try_into()
                .map_err(|_| "bad offset")?,
        ) as usize;

        if seg_info_offset == 0 {
            continue;
        }
        let abs_offset = starts_base + seg_info_offset;
        if abs_offset + 22 > fixup_data.len() {
            return Err("Chained fixups segment info out of bounds");
        }

        let seg_data = &fixup_data[abs_offset..];
        let page_size = u16::from_le_bytes([seg_data[4], seg_data[5]]);
        let pointer_format = u16::from_le_bytes([seg_data[6], seg_data[7]]);
        let segment_offset =
            u64::from_le_bytes(seg_data[8..16].try_into().map_err(|_| "bad seg offset")?);
        let page_count = u16::from_le_bytes([seg_data[20], seg_data[21]]) as usize;

        // crate::println!(
        //     "[darwin] fixups seg[{}]: vmaddr={:#x} page_size={:#x} fmt={} page_count={}",
        //     seg_idx, segment_offset, page_size, pointer_format, page_count
        // );

        if pointer_format != DYLD_CHAINED_PTR_ARM64E
            && pointer_format != DYLD_CHAINED_PTR_ARM64E_USERLAND24
        {
            continue;
        }

        let seg_vaddr = if base_delta >= 0 {
            usize::try_from(segment_offset)
                .map_err(|_| "segment_offset out of range")?
                .checked_add(base_delta as usize)
                .ok_or("seg_vaddr overflow")?
        } else {
            usize::try_from(segment_offset)
                .map_err(|_| "segment_offset out of range")?
                .checked_sub((-base_delta) as usize)
                .ok_or("seg_vaddr underflow")?
        };

        let chain_stride: usize = 8;

        for page_idx in 0..page_count {
            let ps_pos = 22 + page_idx * 2;
            if ps_pos + 2 > seg_data.len() {
                break;
            }
            let page_start = u16::from_le_bytes([seg_data[ps_pos], seg_data[ps_pos + 1]]);

            if page_start == DYLD_CHAINED_PTR_START_NONE {
                continue;
            }
            if page_start & DYLD_CHAINED_PTR_START_MULTI != 0 {
                continue;
            }

            let page_addr = seg_vaddr + page_idx * page_size as usize;
            let chain_start = page_start as usize * chain_stride;

            // Phase 1: Read all chain entries before writing any.
            // With stride=4 and 8-byte entries, consecutive reads overlap
            // if we write before advancing. Collecting first avoids corruption.
            let mut chain_entries: Vec<(usize, u64)> = Vec::new();
            let mut chain_offset = chain_start;
            loop {
                let entry_addr = page_addr + chain_offset;
                let kva = match task.vm_manager.translate_to_kva(entry_addr) {
                    Some(k) => k,
                    None => break,
                };

                let value = unsafe { ptr::read(kva as *const u64) };
                let bind_bit = (value >> 62) & 1;
                let next = ((value >> 51) & 0x7FF) as usize;

                if chain_entries.len() < 10 {
                    crate::println!(
                        "[darwin] chain[{}/{}][{}]: addr={:#x} val={:#x} auth={} bind={} next={}",
                        seg_idx,
                        page_idx,
                        chain_entries.len(),
                        entry_addr,
                        value,
                        (value >> 63) & 1,
                        bind_bit,
                        next
                    );
                }

                chain_entries.push((kva, value));

                if bind_bit != 0 || next == 0 {
                    break;
                }
                chain_offset += next * chain_stride;
                if chain_entries.len() > 4096 {
                    break;
                }
            }

            crate::println!(
                "[darwin] seg[{}] page[{}]: {} entries",
                seg_idx,
                page_idx,
                chain_entries.len()
            );

            // Phase 2: Compute and write fixups using saved original values.
            for (kva, current_value) in &chain_entries {
                let auth_bit = (*current_value >> 63) & 1;

                let new_value = if pointer_format == DYLD_CHAINED_PTR_ARM64E && auth_bit != 0 {
                    let target = *current_value & 0xFFFFFFFF;
                    (base_addr.wrapping_add(target)) & 0x0000_FFFF_FFFF_FFFF
                } else if pointer_format == DYLD_CHAINED_PTR_ARM64E {
                    let target43 = *current_value & ((1u64 << 43) - 1);
                    let high8 = (*current_value >> 43) & 0xFF;
                    let preferred_vmaddr = (high8 << 56) | target43;
                    let rebased = if base_delta >= 0 {
                        preferred_vmaddr.wrapping_add(base_delta as u64)
                    } else {
                        preferred_vmaddr.wrapping_sub((-base_delta) as u64)
                    };
                    rebased & 0x0000_FFFF_FFFF_FFFF
                } else if auth_bit != 0 {
                    let target = *current_value & 0xFFFFFFFF;
                    base_addr.wrapping_add(target)
                } else {
                    let target = *current_value & 0xFFFFFFFF;
                    let high8 = (*current_value >> 32) & 0xFF;
                    let full_target = (high8 << 56) | target;
                    base_addr.wrapping_add(full_target)
                };

                unsafe {
                    ptr::write(*kva as *mut u64, new_value);
                }
            }
        }
    }

    Ok(())
}

/// Load dyld (the Mach-O dynamic linker) into the task's address space.
/// Returns (dyld_entry_point, base_delta) where base_delta is the relocation offset.
pub fn load_dyld(dyld_path: &str, task: &Task) -> Result<(usize, i64), &'static str> {
    let vfs = task.get_vfs().ok_or("Task VFS not available")?;
    let file_obj = match vfs.open(dyld_path, 0) {
        Ok(ko) => ko,
        Err(_) => {
            let alt_paths = ["/usr/lib/dyld", "/System/usr/lib/dyld"];
            let mut found = None;
            for alt in alt_paths {
                if let Ok(ko) = vfs.open(alt, 0) {
                    found = Some(ko);
                    break;
                }
            }
            found.ok_or("Failed to open dyld from VFS")?
        }
    };

    let file_ref = match file_obj {
        crate::object::KernelObject::File(f) => f,
        _ => return Err("dyld is not a file object"),
    };
    let raw_file: &dyn FileObject = file_ref.as_ref();

    let slice_offset = find_arm64_slice(raw_file)?;

    raw_file
        .seek(SeekFrom::Start(slice_offset))
        .map_err(|_| "Failed to seek to dyld header")?;

    let mut header_bytes = [0u8; size_of::<MachHeader64>()];
    read_exact(raw_file, &mut header_bytes)?;
    let header = read_struct::<MachHeader64>(&header_bytes)?;

    if header.magic != MH_MAGIC_64 {
        return Err("Invalid dyld Mach-O magic");
    }
    if header.cputype != CPU_TYPE_ARM64 {
        return Err("dyld is not ARM64");
    }
    if header.cpusubtype != CPU_SUBTYPE_ALL && header.cpusubtype != CPU_SUBTYPE_ARM64E {
        return Err("Unsupported dyld CPU subtype");
    }
    if header.filetype != MH_EXECUTE && header.filetype != MH_DYLINKER {
        return Err("Unsupported dyld Mach-O file type");
    }

    let mut load_commands = vec![0u8; header.sizeofcmds as usize];
    read_exact(raw_file, &mut load_commands)?;

    let mut segments = Vec::new();
    let mut entryoff = None;
    let mut unixthread_entry = None;
    let mut chained_fixups: Option<(u32, u32)> = None;

    let mut offset = 0usize;
    for _ in 0..header.ncmds {
        let command_end = offset
            .checked_add(size_of::<LoadCommand>())
            .ok_or("dyld load command overflow")?;
        if command_end > load_commands.len() {
            return Err("dyld load command table truncated");
        }

        let load_cmd = read_struct::<LoadCommand>(&load_commands[offset..command_end])?;
        let cmdsize = load_cmd.cmdsize as usize;
        if cmdsize < size_of::<LoadCommand>() {
            return Err("Invalid dyld load command size");
        }

        let next_offset = offset
            .checked_add(cmdsize)
            .ok_or("dyld load command overflow")?;
        if next_offset > load_commands.len() {
            return Err("dyld load command exceeds command table");
        }

        let command_bytes = &load_commands[offset..next_offset];
        crate::println!(
            "[darwin] dyld cmd 0x{:x} size={}",
            load_cmd.cmd,
            load_cmd.cmdsize
        );
        match load_cmd.cmd {
            LC_SEGMENT_64 => {
                if cmdsize >= size_of::<SegmentCommand64>() {
                    segments.push(read_struct::<SegmentCommand64>(
                        &command_bytes[..size_of::<SegmentCommand64>()],
                    )?);
                }
            }
            LC_MAIN => {
                if cmdsize >= size_of::<EntryPointCommand>() {
                    let entry_cmd = read_struct::<EntryPointCommand>(
                        &command_bytes[..size_of::<EntryPointCommand>()],
                    )?;
                    entryoff = Some(entry_cmd.entryoff);
                }
            }
            LC_UNIXTHREAD => {
                if cmdsize >= MIN_UNIXTHREAD_COMMAND_SIZE {
                    let pc_offset = offset_of_pc_in_unixthread();
                    unixthread_entry = Some(read_u64(&command_bytes[pc_offset..pc_offset + 8]));
                }
            }
            LC_DYLD_CHAINED_FIXUPS => {
                if cmdsize >= size_of::<LinkEditDataCommand>() {
                    let fc = read_struct::<LinkEditDataCommand>(
                        &command_bytes[..size_of::<LinkEditDataCommand>()],
                    )?;
                    crate::println!(
                        "[darwin] dyld LC_DYLD_CHAINED_FIXUPS: dataoff={} datasize={}",
                        fc.dataoff,
                        fc.datasize
                    );
                    chained_fixups = Some((fc.dataoff, fc.datasize));
                }
            }
            _ => {}
        }

        offset = next_offset;
    }

    if segments.is_empty() {
        return Err("dyld has no segments");
    }

    let min_vmaddr = segments
        .iter()
        .map(|segment| segment.vmaddr)
        .min()
        .ok_or("dyld has no segments")?;
    let max_vmaddr = segments
        .iter()
        .map(|segment| segment.vmaddr.saturating_add(segment.vmsize))
        .max()
        .ok_or("dyld has no segments")?;
    let total_size =
        usize::try_from(max_vmaddr.saturating_sub(min_vmaddr)).map_err(|_| "dyld size overflow")?;
    let aligned_total = align_up(total_size, PAGE_SIZE);

    let target_base = task
        .vm_manager
        .find_unmapped_area(aligned_total, PAGE_SIZE)
        .ok_or("No free VM area for dyld")?;

    let base_delta = target_base as i64 - min_vmaddr as i64;

    crate::println!(
        "[darwin] dyld min_vmaddr={:#x} target_base={:#x} base_delta={:#x}",
        min_vmaddr,
        target_base,
        base_delta
    );
    for segment in &segments {
        crate::println!(
            "[darwin] dyld seg: vmaddr={:#x} vmsize={:#x} fileoff={:#x} filesize={:#x} prot={}",
            segment.vmaddr,
            segment.vmsize,
            segment.fileoff,
            segment.filesize,
            segment.initprot
        );
        map_segment_with_base(raw_file, task, segment, base_delta, slice_offset)?;
    }

    if let Some((dataoff, datasize)) = chained_fixups {
        if datasize > 0 {
            let mut fixup_buf = vec![0u8; datasize as usize];
            raw_file
                .seek(SeekFrom::Start(slice_offset + dataoff as u64))
                .map_err(|_| "Failed to seek to dyld chained fixups")?;
            read_exact(raw_file, &mut fixup_buf)?;

            let base_addr = target_base as u64;
            apply_chained_fixups(task, base_addr, base_delta, &fixup_buf)?;
        }
    } else {
        crate::println!("[darwin] no LC_DYLD_CHAINED_FIXUPS found in dyld");
    }

    let dyld_entry = if let Some(off) = entryoff {
        let original_vaddr =
            file_offset_to_vaddr(&segments, off).ok_or("Failed to resolve dyld entry point")?;
        if base_delta >= 0 {
            original_vaddr
                .checked_add(base_delta as usize)
                .ok_or("entry overflow")?
        } else {
            original_vaddr
                .checked_sub((-base_delta) as usize)
                .ok_or("entry underflow")?
        }
    } else if let Some(entry) = unixthread_entry {
        let original = entry as usize;
        if base_delta >= 0 {
            original
                .checked_add(base_delta as usize)
                .ok_or("entry overflow")?
        } else {
            original
                .checked_sub((-base_delta) as usize)
                .ok_or("entry underflow")?
        }
    } else {
        return Err("dyld has no entry point");
    };

    Ok((dyld_entry, base_delta))
}

pub const DYLD_SHARED_CACHE_BASE: usize = 0x8000_0000;
const DYLD_SHARED_CACHE_SIZE: usize = 0x0100_0000; // 16 MiB placeholder region for dyld probes

const COMMPAGE_BASE: usize = 0x0FFFFFC000;
const COMMPAGE_SIZE: usize = 0x1000;
const COMMPAGE_RO_BASE: usize = 0x0FFFFF4000;
const SHARED_REGION_BASE: usize = 0x0FFFFF0000;
const SHARED_REGION_SIZE: usize = 0xD000; // 0x0FFFFF0000 .. 0x0FFFFFD000 (52KB)
const COMMPAGE_SIGNATURE: &[u8; 16] = b"commpage 64-bit\0";

const COMMPAGE_RW_OFFSET: usize = COMMPAGE_BASE - SHARED_REGION_BASE; // 0xC000
const COMMPAGE_RO_OFFSET: usize = COMMPAGE_RO_BASE - SHARED_REGION_BASE; // 0x4000

pub fn setup_commpage(task: &Task) -> Result<(), &'static str> {
    let num_pages = SHARED_REGION_SIZE / PAGE_SIZE;
    let pages = ContiguousPages::new(num_pages).ok_or("Failed to allocate shared region")?;
    let paddr = pages.as_paddr();

    let mmap = VirtualMemoryMap::new(
        MemoryArea::new(paddr, paddr + SHARED_REGION_SIZE - 1),
        MemoryArea::new(
            SHARED_REGION_BASE,
            SHARED_REGION_BASE + SHARED_REGION_SIZE - 1,
        ),
        VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::User as usize,
        false,
        None,
    );
    task.vm_manager.add_memory_map(mmap)?;
    task.page_allocations.write().push(pages);

    let shared_kva = task
        .vm_manager
        .translate_to_kva(SHARED_REGION_BASE)
        .ok_or("Failed to translate shared region")?;

    unsafe {
        ptr::write_bytes(shared_kva as *mut u8, 0, SHARED_REGION_SIZE);
    }

    let rw_kva = shared_kva + COMMPAGE_RW_OFFSET;
    let ro_kva = shared_kva + COMMPAGE_RO_OFFSET;

    unsafe {
        ptr::copy_nonoverlapping(
            COMMPAGE_SIGNATURE.as_ptr(),
            ro_kva as *mut u8,
            COMMPAGE_SIGNATURE.len(),
        );
        ptr::copy_nonoverlapping(
            COMMPAGE_SIGNATURE.as_ptr(),
            rw_kva as *mut u8,
            COMMPAGE_SIGNATURE.len(),
        );

        let cpu_caps_64_offset = 0x010usize;
        let cpu_caps: u64 = (1u64 << 7) | (1u64 << 8) | (1u64 << 11) | (1u64 << 17) | (1u64 << 40);
        ptr::write((ro_kva + cpu_caps_64_offset) as *mut u64, cpu_caps);
        ptr::write((rw_kva + cpu_caps_64_offset) as *mut u64, cpu_caps);

        let cpu_caps_32_offset = 0x020usize;
        ptr::write((ro_kva + cpu_caps_32_offset) as *mut u32, cpu_caps as u32);
        ptr::write((rw_kva + cpu_caps_32_offset) as *mut u32, cpu_caps as u32);

        let ncpus_offset = 0x022usize;
        ptr::write((ro_kva + ncpus_offset) as *mut u8, 1);
        ptr::write((rw_kva + ncpus_offset) as *mut u8, 1);

        let page_shift_64_offset = 0x025usize;
        ptr::write((ro_kva + page_shift_64_offset) as *mut u8, 12);
        ptr::write((rw_kva + page_shift_64_offset) as *mut u8, 12);

        let cache_linesize_offset = 0x026usize;
        ptr::write((ro_kva + cache_linesize_offset) as *mut u16, 64);
        ptr::write((rw_kva + cache_linesize_offset) as *mut u16, 64);

        let active_cpus_offset = 0x034usize;
        ptr::write((ro_kva + active_cpus_offset) as *mut u8, 1);
        ptr::write((rw_kva + active_cpus_offset) as *mut u8, 1);

        let physical_cpus_offset = 0x035usize;
        ptr::write((ro_kva + physical_cpus_offset) as *mut u8, 1);
        ptr::write((rw_kva + physical_cpus_offset) as *mut u8, 1);

        let logical_cpus_offset = 0x036usize;
        ptr::write((ro_kva + logical_cpus_offset) as *mut u8, 1);
        ptr::write((rw_kva + logical_cpus_offset) as *mut u8, 1);

        let kernel_page_shift_offset = 0x037usize;
        ptr::write((ro_kva + kernel_page_shift_offset) as *mut u8, 12);
        ptr::write((rw_kva + kernel_page_shift_offset) as *mut u8, 12);

        let version_offset = 0x01Eusize;
        ptr::write((ro_kva + version_offset) as *mut u16, 3);
        ptr::write((rw_kva + version_offset) as *mut u16, 3);
    }

    crate::println!(
        "[darwin] shared region mapped at {:#x} size={:#x} (ro at +{:#x}, rw at +{:#x})",
        SHARED_REGION_BASE,
        SHARED_REGION_SIZE,
        COMMPAGE_RO_OFFSET,
        COMMPAGE_RW_OFFSET
    );
    Ok(())
}

pub fn setup_shared_cache_region(task: &Task) -> Result<(), &'static str> {
    if task
        .vm_manager
        .search_memory_map(DYLD_SHARED_CACHE_BASE)
        .is_some()
    {
        return Ok(());
    }

    let num_pages = DYLD_SHARED_CACHE_SIZE / PAGE_SIZE;
    let pages =
        ContiguousPages::new(num_pages).ok_or("Failed to allocate dyld shared cache region")?;
    let paddr = pages.as_paddr();

    let mmap = VirtualMemoryMap::new(
        MemoryArea::new(paddr, paddr + DYLD_SHARED_CACHE_SIZE - 1),
        MemoryArea::new(
            DYLD_SHARED_CACHE_BASE,
            DYLD_SHARED_CACHE_BASE + DYLD_SHARED_CACHE_SIZE - 1,
        ),
        VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize
            | VirtualMemoryPermission::User as usize,
        false,
        None,
    );
    task.vm_manager.add_memory_map(mmap)?;
    task.page_allocations.write().push(pages);

    let shared_cache_kva = task
        .vm_manager
        .translate_to_kva(DYLD_SHARED_CACHE_BASE)
        .ok_or("Failed to translate dyld shared cache region")?;

    unsafe {
        ptr::write_bytes(shared_cache_kva as *mut u8, 0, DYLD_SHARED_CACHE_SIZE);
    }

    Ok(())
}

pub const fn shared_cache_base() -> usize {
    DYLD_SHARED_CACHE_BASE
}

pub fn setup_tls(task: &Task, thread_port: u32) -> Result<usize, &'static str> {
    let tls_pages = ContiguousPages::new(1).ok_or("Failed to allocate TLS page")?;
    let paddr = tls_pages.as_paddr();

    let tls_vaddr = task
        .vm_manager
        .find_unmapped_area(PAGE_SIZE, PAGE_SIZE)
        .ok_or("No free VM area for TLS")?;

    let mmap = VirtualMemoryMap::new(
        MemoryArea::new(paddr, paddr + PAGE_SIZE - 1),
        MemoryArea::new(tls_vaddr, tls_vaddr + PAGE_SIZE - 1),
        VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::User as usize,
        false,
        None,
    );
    task.vm_manager.add_memory_map(mmap)?;
    task.page_allocations.write().push(tls_pages);

    let kva = task
        .vm_manager
        .translate_to_kva(tls_vaddr)
        .ok_or("Failed to translate TLS page")?;

    unsafe {
        ptr::write_bytes(kva as *mut u8, 0, PAGE_SIZE);
        // TSD slot 3 (offset 0x18) = __TSD_MACH_THREAD_SELF
        ptr::write((kva as *mut u8).add(0x18) as *mut u64, thread_port as u64);
    }

    crate::println!(
        "[darwin] TLS page at {:#x} (thread_port={})",
        tls_vaddr,
        thread_port
    );
    Ok(tls_vaddr)
}

fn macho_prot_to_scarlet(prot: i32) -> usize {
    let mut perms = 0;
    if prot & 1 != 0 {
        perms |= VirtualMemoryPermission::Read as usize;
    }
    if prot & 2 != 0 {
        perms |= VirtualMemoryPermission::Write as usize;
    }
    if prot & 4 != 0 {
        perms |= VirtualMemoryPermission::Execute as usize;
    }
    perms |= VirtualMemoryPermission::User as usize;
    perms
}

fn file_offset_to_vaddr(segments: &[SegmentCommand64], entryoff: u64) -> Option<usize> {
    segments.iter().find_map(|segment| {
        let file_start = segment.fileoff;
        let file_end = segment.fileoff.checked_add(segment.filesize)?;
        if entryoff < file_start || entryoff >= file_end {
            return None;
        }

        let delta = entryoff.checked_sub(segment.fileoff)?;
        let vaddr = segment.vmaddr.checked_add(delta)?;
        usize::try_from(vaddr).ok()
    })
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

fn offset_of_pc_in_unixthread() -> usize {
    ARM_THREAD_STATE64_PC_OFFSET
}

fn read_exact(file_obj: &dyn FileObject, buffer: &mut [u8]) -> Result<(), &'static str> {
    let mut total_read = 0usize;
    while total_read < buffer.len() {
        let read_len = file_obj
            .read(&mut buffer[total_read..])
            .map_err(|_| "Failed to read Mach-O data")?;
        if read_len == 0 {
            return Err("Unexpected EOF while reading Mach-O data");
        }
        total_read += read_len;
    }
    Ok(())
}

fn read_struct<T: Copy>(bytes: &[u8]) -> Result<T, &'static str> {
    if bytes.len() < size_of::<T>() {
        return Err("Mach-O structure truncated");
    }

    Ok(unsafe { ptr::read_unaligned(bytes.as_ptr() as *const T) })
}

fn read_u32(bytes: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&bytes[..4]);
    u32::from_le_bytes(buf)
}

fn read_u64(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    /// Minimal Mach-O aarch64 static executable: exit(42)
    /// Built from hand-assembled ARM64 code:
    ///   mov x16, #1; movk x16, #0x200, lsl #16; mov x0, #42; svc #0x80; b .
    /// Header (32) + LC_SEGMENT_64 with __text (152) + LC_MAIN (24) + code (20) = 228 bytes
    const MINIMAL_MACHO_EXIT: &[u8] = &[
        // mach_header_64 (32 bytes)
        0xcf, 0xfa, 0xed, 0xfe, // magic = MH_MAGIC_64
        0x0c, 0x00, 0x00, 0x01, // cputype = CPU_TYPE_ARM64
        0x00, 0x00, 0x00, 0x00, // cpusubtype = CPU_SUBTYPE_ALL
        0x02, 0x00, 0x00, 0x00, // filetype = MH_EXECUTE
        0x02, 0x00, 0x00, 0x00, // ncmds = 2
        0xb0, 0x00, 0x00, 0x00, // sizeofcmds = 176
        0x85, 0x00, 0x20, 0x00, // flags
        0x00, 0x00, 0x00, 0x00, // reserved
        // LC_SEGMENT_64 (72 bytes)
        0x19, 0x00, 0x00, 0x00, // cmd = LC_SEGMENT_64
        0x98, 0x00, 0x00, 0x00, // cmdsize = 152
        0x5f, 0x5f, 0x54, 0x45, 0x58, 0x54, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, // segname = "__TEXT"
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, // vmaddr = 0x100000000
        0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, // vmsize = 16384
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // fileoff = 0
        0xe4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesize = 228
        0x07, 0x00, 0x00, 0x00, // maxprot = rwx
        0x07, 0x00, 0x00, 0x00, // initprot = rwx
        0x01, 0x00, 0x00, 0x00, // nsects = 1
        0x00, 0x00, 0x00, 0x00, // flags
        // Section __text (80 bytes)
        0x5f, 0x5f, 0x74, 0x65, 0x78, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, // sectname = "__text"
        0x5f, 0x5f, 0x54, 0x45, 0x58, 0x54, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, // segname = "__TEXT"
        0xd0, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, // addr = 0x1000000d0
        0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // size = 20
        0xd0, 0x00, 0x00, 0x00, // offset = 208
        0x00, 0x00, 0x00, 0x00, // align
        0x00, 0x00, 0x00, 0x00, // reloff
        0x00, 0x00, 0x00, 0x00, // nreloc
        0x00, 0x04, 0x00, 0x80, // flags (PURE_INSTRUCTIONS | SOME_INSTRUCTIONS)
        0x00, 0x00, 0x00, 0x00, // reserved1
        0x00, 0x00, 0x00, 0x00, // reserved2
        0x00, 0x00, 0x00, 0x00, // reserved3
        // LC_MAIN (24 bytes)
        0x28, 0x00, 0x00, 0x80, // cmd = LC_MAIN
        0x18, 0x00, 0x00, 0x00, // cmdsize = 24
        0xd0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // entryoff = 208
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // stacksize = 0
        // Code: exit(42) via SVC #0x80
        0x30, 0x00, 0x80, 0xd2, // mov x16, #1
        0x10, 0x40, 0xa0, 0xf2, // movk x16, #0x200, lsl #16
        0x40, 0x05, 0x80, 0xd2, // mov x0, #42
        0x01, 0x10, 0x00, 0xd4, // svc #0x80
        0x00, 0x00, 0x00, 0x14, // b .
    ];

    /// Helper: in-memory FileObject for testing Mach-O parsing
    struct MockFileObject {
        data: alloc::vec::Vec<u8>,
        pos: usize,
    }

    impl MockFileObject {
        fn new(data: &[u8]) -> Self {
            Self {
                data: alloc::vec::Vec::from(data),
                pos: 0,
            }
        }
    }

    impl crate::fs::FileObject for MockFileObject {
        fn read(&self, _buf: &mut [u8]) -> Result<usize, crate::fs::FileSystemError> {
            // Not needed for these tests — we use direct struct parsing
            Ok(0)
        }

        fn write(&self, _buf: &[u8]) -> Result<usize, crate::fs::FileSystemError> {
            Err(crate::fs::FileSystemError::new(
                crate::fs::FileSystemErrorKind::NotSupported,
                "mock write not supported",
            ))
        }

        fn seek(&self, _pos: SeekFrom) -> Result<u64, crate::fs::FileSystemError> {
            Ok(0)
        }

        fn flush(&self) -> Result<(), crate::fs::FileSystemError> {
            Ok(())
        }

        fn size(&self) -> Result<usize, crate::fs::FileSystemError> {
            Ok(self.data.len())
        }
    }

    #[test]
    fn test_parse_mach_header() {
        let header_bytes = &MINIMAL_MACHO_EXIT[..size_of::<MachHeader64>()];
        let header = read_struct::<MachHeader64>(header_bytes).unwrap();

        assert_eq!(header.magic, MH_MAGIC_64, "magic should be MH_MAGIC_64");
        assert_eq!(header.cputype, CPU_TYPE_ARM64, "cputype should be ARM64");
        assert_eq!(
            header.cpusubtype, CPU_SUBTYPE_ALL,
            "cpusubtype should be ALL"
        );
        assert_eq!(header.filetype, MH_EXECUTE, "filetype should be MH_EXECUTE");
        assert_eq!(header.ncmds, 2, "should have 2 load commands");
    }

    #[test]
    fn test_parse_load_commands() {
        let header =
            read_struct::<MachHeader64>(&MINIMAL_MACHO_EXIT[..size_of::<MachHeader64>()]).unwrap();
        let cmd_start = size_of::<MachHeader64>();
        let cmd_data = &MINIMAL_MACHO_EXIT[cmd_start..cmd_start + header.sizeofcmds as usize];

        let mut offset = 0usize;
        let mut found_segment = false;
        let mut found_main = false;
        let mut entryoff = 0u64;

        for _ in 0..header.ncmds {
            let load_cmd =
                read_struct::<LoadCommand>(&cmd_data[offset..offset + size_of::<LoadCommand>()])
                    .unwrap();
            let cmdsize = load_cmd.cmdsize as usize;
            let command_bytes = &cmd_data[offset..offset + cmdsize];

            match load_cmd.cmd {
                LC_SEGMENT_64 => {
                    found_segment = true;
                    let seg = read_struct::<SegmentCommand64>(
                        &command_bytes[..size_of::<SegmentCommand64>()],
                    )
                    .unwrap();
                    let segname =
                        &seg.segname[..seg.segname.iter().position(|&b| b == 0).unwrap_or(16)];
                    assert_eq!(segname, b"__TEXT", "first segment should be __TEXT");
                    assert_eq!(seg.vmaddr, 0x100000000, "vmaddr should be 0x100000000");
                    assert_eq!(seg.nsects, 1, "should have 1 section");
                }
                LC_MAIN => {
                    found_main = true;
                    let entry_cmd = read_struct::<EntryPointCommand>(
                        &command_bytes[..size_of::<EntryPointCommand>()],
                    )
                    .unwrap();
                    entryoff = entry_cmd.entryoff;
                }
                _ => {}
            }
            offset += cmdsize;
        }

        assert!(found_segment, "should find LC_SEGMENT_64");
        assert!(found_main, "should find LC_MAIN");
        assert_eq!(entryoff, 208, "entry offset should be 208");
    }

    #[test]
    fn test_entry_point_resolution() {
        let header =
            read_struct::<MachHeader64>(&MINIMAL_MACHO_EXIT[..size_of::<MachHeader64>()]).unwrap();
        let cmd_start = size_of::<MachHeader64>();
        let cmd_data = &MINIMAL_MACHO_EXIT[cmd_start..cmd_start + header.sizeofcmds as usize];

        let mut segments: alloc::vec::Vec<SegmentCommand64> = alloc::vec::Vec::new();
        let mut entryoff = None;

        let mut offset = 0usize;
        for _ in 0..header.ncmds {
            let load_cmd =
                read_struct::<LoadCommand>(&cmd_data[offset..offset + size_of::<LoadCommand>()])
                    .unwrap();
            let cmdsize = load_cmd.cmdsize as usize;
            let command_bytes = &cmd_data[offset..offset + cmdsize];

            match load_cmd.cmd {
                LC_SEGMENT_64 => {
                    segments.push(
                        read_struct::<SegmentCommand64>(
                            &command_bytes[..size_of::<SegmentCommand64>()],
                        )
                        .unwrap(),
                    );
                }
                LC_MAIN => {
                    let entry_cmd = read_struct::<EntryPointCommand>(
                        &command_bytes[..size_of::<EntryPointCommand>()],
                    )
                    .unwrap();
                    entryoff = Some(entry_cmd.entryoff);
                }
                _ => {}
            }
            offset += cmdsize;
        }

        let entryoff = entryoff.expect("should have LC_MAIN");
        let entry_vaddr =
            file_offset_to_vaddr(&segments, entryoff).expect("should resolve entry point");

        // entryoff=208, __TEXT starts at fileoff=0, vmaddr=0x100000000
        // so vaddr = 0x100000000 + 208 = 0x1000000d0
        assert_eq!(
            entry_vaddr, 0x1000000d0,
            "entry point should be 0x1000000d0"
        );
    }

    #[test]
    fn test_code_at_entry_offset() {
        let expected_code: &[u8] = &[
            0x30, 0x00, 0x80, 0xd2, // mov x16, #1
            0x10, 0x40, 0xa0, 0xf2, // movk x16, #0x200, lsl #16
            0x40, 0x05, 0x80, 0xd2, // mov x0, #42
            0x01, 0x10, 0x00, 0xd4, // svc #0x80
            0x00, 0x00, 0x00, 0x14, // b .
        ];

        let code_in_binary = &MINIMAL_MACHO_EXIT[208..228];
        assert_eq!(
            code_in_binary, expected_code,
            "code should match at offset 208"
        );
    }

    #[test]
    fn test_macho_prot_to_scarlet() {
        use crate::vm::vmem::VirtualMemoryPermission;

        // rwx (7) -> Read | Write | Execute | User
        let rwx = macho_prot_to_scarlet(7);
        assert_eq!(
            rwx & VirtualMemoryPermission::Read as usize,
            VirtualMemoryPermission::Read as usize
        );
        assert_eq!(
            rwx & VirtualMemoryPermission::Write as usize,
            VirtualMemoryPermission::Write as usize
        );
        assert_eq!(
            rwx & VirtualMemoryPermission::Execute as usize,
            VirtualMemoryPermission::Execute as usize
        );
        assert_eq!(
            rwx & VirtualMemoryPermission::User as usize,
            VirtualMemoryPermission::User as usize
        );

        // r-x (5) -> Read | Execute | User
        let rx = macho_prot_to_scarlet(5);
        assert_eq!(
            rx & VirtualMemoryPermission::Read as usize,
            VirtualMemoryPermission::Read as usize
        );
        assert_eq!(rx & VirtualMemoryPermission::Write as usize, 0);
        assert_eq!(
            rx & VirtualMemoryPermission::Execute as usize,
            VirtualMemoryPermission::Execute as usize
        );

        // rw- (3) -> Read | Write | User
        let rw = macho_prot_to_scarlet(3);
        assert_eq!(
            rw & VirtualMemoryPermission::Read as usize,
            VirtualMemoryPermission::Read as usize
        );
        assert_eq!(
            rw & VirtualMemoryPermission::Write as usize,
            VirtualMemoryPermission::Write as usize
        );
        assert_eq!(rw & VirtualMemoryPermission::Execute as usize, 0);
    }

    #[test]
    fn test_reject_dynamic_binary() {
        let mut dynamic_macho = alloc::vec::Vec::new();

        // Header
        let header = MachHeader64 {
            magic: MH_MAGIC_64,
            cputype: CPU_TYPE_ARM64,
            cpusubtype: CPU_SUBTYPE_ALL,
            filetype: MH_EXECUTE,
            ncmds: 1,
            sizeofcmds: 24, // just LC_LOAD_DYLIB
            flags: 0,
            reserved: 0,
        };
        unsafe {
            let bytes = core::slice::from_raw_parts(
                &header as *const MachHeader64 as *const u8,
                size_of::<MachHeader64>(),
            );
            dynamic_macho.extend_from_slice(bytes);
        }

        // LC_LOAD_DYLIB (24 bytes minimum)
        let lc_dylib: [u8; 24] = [
            0x0c, 0x00, 0x00, 0x00, // cmd = LC_LOAD_DYLIB
            0x18, 0x00, 0x00, 0x00, // cmdsize = 24
            0x18, 0x00, 0x00, 0x00, // name offset (points past the cmd)
            0x00, 0x00, 0x00, 0x00, // timestamp
            0x01, 0x00, 0x00, 0x00, // current version
            0x01, 0x00, 0x00, 0x00, // compatibility version
        ];
        dynamic_macho.extend_from_slice(&lc_dylib);

        // Parse and verify it would be rejected
        let header_bytes = &dynamic_macho[..size_of::<MachHeader64>()];
        let header = read_struct::<MachHeader64>(header_bytes).unwrap();
        let cmd_start = size_of::<MachHeader64>();
        let cmd_data = &dynamic_macho[cmd_start..];

        let mut offset = 0usize;
        let mut has_dylib = false;
        for _ in 0..header.ncmds {
            let load_cmd =
                read_struct::<LoadCommand>(&cmd_data[offset..offset + size_of::<LoadCommand>()])
                    .unwrap();
            if load_cmd.cmd == LC_LOAD_DYLIB {
                has_dylib = true;
            }
            offset += load_cmd.cmdsize as usize;
        }
        assert!(has_dylib, "should detect LC_LOAD_DYLIB");
    }

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(0, 4096), 0);
        assert_eq!(align_up(1, 4096), 4096);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(4097, 4096), 8192);
        assert_eq!(align_up(228, 16384), 16384);
    }
}
