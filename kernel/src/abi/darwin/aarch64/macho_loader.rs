use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};
use core::sync::atomic::Ordering;
use core::{mem::size_of, ptr};
use spin::{Mutex, Once};

use crate::{
    environment::PAGE_SIZE,
    fs::vfs_v2::manager::VfsManager,
    fs::{FileObject, SeekFrom},
    mem::page::ContiguousPages,
    object::capability::memory_mapping::{
        AccessKind, MemoryMappingOps, ResolveFaultError, ResolveFaultResult,
    },
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

    // Do NOT apply dyld's chained fixups here.
    // On macOS, dyld processes its own fixups at startup. If the kernel
    // pre-applies them, dyld will slide them again → double-slide crash.
    if chained_fixups.is_none() {
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

pub const DYLD_SHARED_CACHE_BASE: usize = 0x1_8000_0000;

/// Cache header field offsets (we read them directly, no full struct)
const CACHE_HEADER_MAPPING_WITH_SLIDE_OFFSET: usize = 0x138;
const CACHE_HEADER_MAPPING_WITH_SLIDE_COUNT: usize = 0x13C;
const CACHE_HEADER_SHARED_REGION_SIZE: usize = 0xE8;

/// dyld_cache_mapping_and_slide_info - 56 bytes each
#[repr(C)]
#[derive(Clone, Copy)]
struct CacheMappingSlideInfo {
    address: u64,
    size: u64,
    file_offset: u64,
    slide_info_file_offset: u64,
    slide_info_file_size: u64,
    flags: u64,
    max_prot: u32,
    init_prot: u32,
}

const CACHE_MAPPING_AUTH_DATA: u64 = 1;

/// v5 slide info header (at slide_info_file_offset in loaded cache data)
/// Layout: version(4) page_size(4) page_starts_count(4) _pad(4) value_add(8) page_starts[]
const SLIDE_INFO5_HEADER_SIZE: usize = 24;

const DYLD_CACHE_SLIDE_V5_PAGE_ATTR_NO_REBASE: u16 = 0xFFFF;

struct SlideInfoMeta {
    mapping_idx: usize,
    file_offset: usize,
    mapping_address: u64,
    mapping_size: usize,
    page_size: u32,
    value_add: u64,
    page_starts: Vec<u16>,
}

struct DarwinSharedCache {
    file: Arc<dyn FileObject>,
    cache_start: usize,
    total_size: usize,
    mapping_count: usize,
    mappings: [CacheMappingSlideInfo; 8],
    slide_infos: Vec<SlideInfoMeta>,
    pages: Mutex<BTreeMap<usize, usize>>,
}

static SHARED_CACHE: Once<Mutex<Option<Arc<DarwinSharedCache>>>> = Once::new();

impl DarwinSharedCache {
    fn find_mapping_for_page(&self, page_vaddr: usize) -> Option<(usize, usize, usize)> {
        for (mapping_idx, mapping) in self.mappings.iter().enumerate().take(self.mapping_count) {
            let mapping_start = usize::try_from(mapping.address).ok()?;
            let mapping_size = usize::try_from(mapping.size).ok()?;
            let mapping_end = mapping_start.checked_add(mapping_size)?;
            if page_vaddr < mapping_start || page_vaddr >= mapping_end {
                continue;
            }

            let offset_in_mapping = page_vaddr - mapping_start;
            let file_offset = usize::try_from(mapping.file_offset).ok()?;
            let page_file_offset = file_offset.checked_add(offset_in_mapping)?;
            return Some((mapping_idx, offset_in_mapping, page_file_offset));
        }

        if page_vaddr >= self.cache_start && page_vaddr < self.cache_start + self.total_size {
            let file_offset = page_vaddr - self.cache_start;
            return Some((self.mapping_count, 0, file_offset));
        }

        None
    }

    fn apply_slide_fixups_for_page(&self, page_index: usize, kva: usize) {
        for si in &self.slide_infos {
            let mapping_start_page = (si.mapping_address as usize - self.cache_start) / PAGE_SIZE;
            let mapping_page_count = si.mapping_size / si.page_size as usize;

            if page_index < mapping_start_page
                || page_index >= mapping_start_page.saturating_add(mapping_page_count)
            {
                continue;
            }

            let local_page_idx = page_index - mapping_start_page;
            if local_page_idx >= si.page_starts.len() {
                continue;
            }

            let page_start = si.page_starts[local_page_idx];
            if page_start == DYLD_CACHE_SLIDE_V5_PAGE_ATTR_NO_REBASE {
                continue;
            }

            let page_data_offset_in_mapping = local_page_idx * si.page_size as usize;
            let mut delta = (page_start / 8) as usize;
            let mut loc = kva as *mut u64;

            loop {
                // SAFETY: `loc` always points within the freshly allocated and loaded page.
                loc = unsafe { loc.add(delta) };
                if loc as usize >= kva + PAGE_SIZE {
                    break;
                }

                // SAFETY: `loc` has been bounds-checked against the page extent.
                let raw = unsafe { ptr::read(loc) };
                let auth_bit = (raw >> 63) & 1;
                let next;

                if auth_bit == 1 {
                    let runtime_offset = raw & 0x3FFFFFFFF;
                    let diversity = (raw >> 34) & 0xFFFF;
                    let addr_div = (raw >> 50) & 1;
                    let key_is_data = (raw >> 51) & 1;
                    next = ((raw >> 52) & 0x7FF) as usize;

                    let target = si.value_add + runtime_offset;
                    let fixup_vaddr = si.mapping_address
                        + page_data_offset_in_mapping as u64
                        + (loc as usize - kva) as u64;
                    let modifier = if addr_div != 0 {
                        diversity ^ (fixup_vaddr >> 3)
                    } else {
                        diversity
                    };

                    let signed_ptr = if key_is_data != 0 {
                        pac_sign_da(target, modifier)
                    } else {
                        pac_sign_ia(target, modifier)
                    };
                    // SAFETY: `loc` points to an in-page u64 slot being fixed up in place.
                    unsafe { ptr::write(loc, signed_ptr) };
                } else {
                    let runtime_offset = raw & 0x3FFFFFFFF;
                    let high8 = (raw >> 34) & 0xFF;
                    next = ((raw >> 44) & 0x7FF) as usize;
                    let target = si.value_add + runtime_offset;
                    // SAFETY: `loc` points to an in-page u64 slot being fixed up in place.
                    unsafe { ptr::write(loc, target | (high8 << 56)) };
                }

                if next == 0 {
                    break;
                }
                delta = next;
            }
        }
    }
}

impl MemoryMappingOps for DarwinSharedCache {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        let perms = VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize
            | VirtualMemoryPermission::User as usize;
        Ok((0, perms, true))
    }

    fn supports_mmap(&self) -> bool {
        true
    }

    fn mmap_owner_name(&self) -> String {
        String::from("darwin_shared_cache")
    }

    fn resolve_fault(
        &self,
        access: &AccessKind,
        map: &VirtualMemoryMap,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let page_vaddr = access.vaddr & !(PAGE_SIZE - 1);
        if page_vaddr < self.cache_start
            || page_vaddr < map.vmarea.start
            || page_vaddr > map.vmarea.end
        {
            return Err(ResolveFaultError::Invalid);
        }

        let page_index = (page_vaddr - self.cache_start) / PAGE_SIZE;

        {
            let pages = self.pages.lock();
            if let Some(&paddr) = pages.get(&page_index) {
                return Ok(ResolveFaultResult {
                    paddr_page_base: paddr,
                    is_tail: false,
                });
            }
        }

        let (_mapping_idx, offset_in_mapping, file_offset) = self
            .find_mapping_for_page(page_vaddr)
            .ok_or(ResolveFaultError::Unmapped)?;
        let mapping_size = if _mapping_idx < self.mapping_count {
            usize::try_from(self.mappings[_mapping_idx].size)
                .map_err(|_| ResolveFaultError::Invalid)?
        } else {
            PAGE_SIZE
        };

        let page = ContiguousPages::new(1).ok_or(ResolveFaultError::Unmapped)?;
        let paddr = page.as_paddr();
        let kva = crate::vm::addr::phys_to_virt(paddr);

        // SAFETY: `kva` is the HHDM-mapped address for a newly allocated physical page.
        unsafe {
            ptr::write_bytes(kva as *mut u8, 0, PAGE_SIZE);
        }

        let readable = core::cmp::min(PAGE_SIZE, mapping_size.saturating_sub(offset_in_mapping));
        let mut total_read = 0usize;
        while total_read < readable {
            // SAFETY: The destination slice stays within the single allocated page.
            let dst = unsafe {
                core::slice::from_raw_parts_mut(
                    (kva + total_read) as *mut u8,
                    readable - total_read,
                )
            };
            let n = self
                .file
                .read_at((file_offset + total_read) as u64, dst)
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            total_read += n;
        }

        self.apply_slide_fixups_for_page(page_index, kva);

        let mut pages = self.pages.lock();
        if let Some(&existing) = pages.get(&page_index) {
            return Ok(ResolveFaultResult {
                paddr_page_base: existing,
                is_tail: false,
            });
        }

        core::mem::forget(page);
        pages.insert(page_index, paddr);
        Ok(ResolveFaultResult {
            paddr_page_base: paddr,
            is_tail: false,
        })
    }
}

fn init_shared_cache(vfs: &VfsManager) -> Result<Arc<DarwinSharedCache>, &'static str> {
    let cache_file_path = "/System/Library/dyld/dyld_shared_cache_arm64e";
    let file_obj = vfs.open(cache_file_path, 0).map_err(|e| {
        crate::println!("[darwin] shared cache open failed: {}", e.message);
        "Cannot open shared cache"
    })?;
    let file = match file_obj {
        crate::object::KernelObject::File(file) => file,
        _ => return Err("Shared cache is not a file"),
    };

    let mut header_buf = vec![0u8; 0x800];
    let mut total_read = 0usize;
    while total_read < header_buf.len() {
        let n = file
            .read_at(total_read as u64, &mut header_buf[total_read..])
            .unwrap_or(0);
        if n == 0 {
            break;
        }
        total_read += n;
    }

    if total_read < 7 || &header_buf[0..7] != b"dyld_v1" {
        return Err("Invalid shared cache magic");
    }
    if total_read < CACHE_HEADER_MAPPING_WITH_SLIDE_COUNT + 4 {
        return Err("Shared cache header truncated");
    }

    let mapping_slide_offset = read_u32(
        &header_buf
            [CACHE_HEADER_MAPPING_WITH_SLIDE_OFFSET..CACHE_HEADER_MAPPING_WITH_SLIDE_OFFSET + 4],
    ) as usize;
    let mapping_slide_count = read_u32(
        &header_buf
            [CACHE_HEADER_MAPPING_WITH_SLIDE_COUNT..CACHE_HEADER_MAPPING_WITH_SLIDE_COUNT + 4],
    ) as usize;
    let mapping_count = mapping_slide_count.min(8);
    if mapping_count == 0 {
        return Err("Shared cache has no mappings");
    }

    let mappings_end = mapping_slide_offset
        .checked_add(mapping_count * 56)
        .ok_or("Shared cache mapping table overflow")?;
    if mappings_end > total_read || mappings_end > header_buf.len() {
        return Err("Shared cache mapping table truncated");
    }

    let mut mappings = [CacheMappingSlideInfo {
        address: 0,
        size: 0,
        file_offset: 0,
        slide_info_file_offset: 0,
        slide_info_file_size: 0,
        flags: 0,
        max_prot: 0,
        init_prot: 0,
    }; 8];
    for (i, mapping) in mappings.iter_mut().enumerate().take(mapping_count) {
        let off = mapping_slide_offset + i * 56;
        let src = &header_buf[off..off + 56];
        mapping.address = read_u64(&src[0..8]);
        mapping.size = read_u64(&src[8..16]);
        mapping.file_offset = read_u64(&src[16..24]);
        mapping.slide_info_file_offset = read_u64(&src[24..32]);
        mapping.slide_info_file_size = read_u64(&src[32..40]);
        mapping.flags = read_u64(&src[40..48]);
        mapping.max_prot = read_u32(&src[48..52]);
        mapping.init_prot = read_u32(&src[52..56]);
    }

    let cache_start =
        usize::try_from(mappings[0].address).map_err(|_| "Shared cache base out of range")?;
    let main_cache_end = mappings
        .iter()
        .take(mapping_count)
        .map(|mapping| mapping.address.saturating_add(mapping.size))
        .max()
        .ok_or("Shared cache has no mappings")?;

    let shared_region_size = if CACHE_HEADER_SHARED_REGION_SIZE + 8 <= header_buf.len() {
        read_u64(&header_buf[CACHE_HEADER_SHARED_REGION_SIZE..CACHE_HEADER_SHARED_REGION_SIZE + 8])
            as usize
    } else {
        0
    };

    let total_size = if shared_region_size > 0 {
        crate::println!(
            "[darwin] shared region: main={:#x} region={:#x} (sub-cache extends {:#x} bytes)",
            usize::try_from(main_cache_end).unwrap_or(0) - cache_start,
            shared_region_size,
            shared_region_size - (usize::try_from(main_cache_end).unwrap_or(0) - cache_start),
        );
        shared_region_size
    } else {
        usize::try_from(main_cache_end)
            .map_err(|_| "Shared cache end out of range")?
            .checked_sub(cache_start)
            .ok_or("Shared cache virtual range underflow")?
    };

    crate::println!(
        "[darwin] shared cache: {}MB at {:#x}",
        total_size / 1024 / 1024,
        cache_start
    );

    let mut slide_infos = Vec::new();
    for (i, mapping) in mappings.iter().enumerate().take(mapping_count) {
        let si_off = mapping.slide_info_file_offset as usize;
        let si_size = mapping.slide_info_file_size as usize;

        if si_size == 0 || si_off == 0 {
            continue;
        }

        let mut slide_header = vec![0u8; SLIDE_INFO5_HEADER_SIZE];
        let mut header_read = 0usize;
        while header_read < slide_header.len() {
            let n = file
                .read_at(
                    (si_off + header_read) as u64,
                    &mut slide_header[header_read..],
                )
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            header_read += n;
        }
        if header_read < slide_header.len() {
            return Err("Shared cache slide info header truncated");
        }

        let version = read_u32(&slide_header[0..4]);
        let page_size = read_u32(&slide_header[4..8]);
        let page_starts_count = read_u32(&slide_header[8..12]) as usize;
        let value_add = read_u64(&slide_header[16..24]);

        if version != 5 {
            return Err("Unsupported shared cache slide info version");
        }
        if page_size == 0 {
            return Err("Shared cache slide info page size is zero");
        }

        let page_starts_bytes = page_starts_count
            .checked_mul(size_of::<u16>())
            .ok_or("Shared cache page_starts overflow")?;
        if SLIDE_INFO5_HEADER_SIZE + page_starts_bytes > si_size {
            return Err("Shared cache page_starts truncated");
        }

        let mut page_starts_raw = vec![0u8; page_starts_bytes];
        let mut starts_read = 0usize;
        while starts_read < page_starts_raw.len() {
            let n = file
                .read_at(
                    (si_off + SLIDE_INFO5_HEADER_SIZE + starts_read) as u64,
                    &mut page_starts_raw[starts_read..],
                )
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            starts_read += n;
        }
        if starts_read < page_starts_raw.len() {
            return Err("Shared cache page_starts read truncated");
        }

        let mut page_starts = Vec::with_capacity(page_starts_count);
        for chunk in page_starts_raw.chunks_exact(size_of::<u16>()) {
            page_starts.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }

        crate::println!(
            "[darwin] slide info [{}]: {} pages, auth_data={}",
            i,
            page_starts_count,
            (mapping.flags & CACHE_MAPPING_AUTH_DATA) != 0
        );

        slide_infos.push(SlideInfoMeta {
            mapping_idx: i,
            file_offset: usize::try_from(mapping.file_offset)
                .map_err(|_| "Shared cache mapping file offset out of range")?,
            mapping_address: mapping.address,
            mapping_size: usize::try_from(mapping.size)
                .map_err(|_| "Shared cache mapping size out of range")?,
            page_size,
            value_add,
            page_starts,
        });
    }

    Ok(Arc::new(DarwinSharedCache {
        file,
        cache_start,
        total_size,
        mapping_count,
        mappings,
        slide_infos,
        pages: Mutex::new(BTreeMap::new()),
    }))
}

pub fn setup_shared_cache_region(task: &Task) -> Result<(), &'static str> {
    let cache = SHARED_CACHE.call_once(|| {
        let vfs = task.get_vfs().expect("No VFS");
        Mutex::new(match init_shared_cache(&vfs) {
            Ok(cache) => {
                crate::println!(
                    "[darwin] shared cache ready: {}MB at {:#x}",
                    cache.total_size / 1024 / 1024,
                    cache.cache_start
                );
                Some(cache)
            }
            Err(e) => {
                crate::println!("[darwin] shared cache init failed: {}", e);
                None
            }
        })
    });

    let guard = cache.lock();
    let cache = guard.as_ref().ok_or("Shared cache not available")?;

    let mmap = VirtualMemoryMap::new(
        MemoryArea::new(0, PAGE_SIZE - 1),
        MemoryArea::new(cache.cache_start, cache.cache_start + cache.total_size - 1),
        VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize
            | VirtualMemoryPermission::User as usize,
        true,
        Some(Arc::downgrade(cache) as alloc::sync::Weak<dyn MemoryMappingOps>),
    );
    task.vm_manager.add_memory_map(mmap)?;

    Ok(())
}

#[inline(always)]
fn pac_sign_ia(ptr: u64, modifier: u64) -> u64 {
    let result;
    unsafe {
        core::arch::asm!(
            ".arch armv8.3-a",
            "pacia {0}, {1}",
            ".arch armv8-a",
            inout(reg) ptr => result,
            in(reg) modifier,
            options(nostack)
        )
    };
    result
}

#[inline(always)]
fn pac_sign_da(ptr: u64, modifier: u64) -> u64 {
    let result;
    unsafe {
        core::arch::asm!(
            ".arch armv8.3-a",
            "pacda {0}, {1}",
            ".arch armv8-a",
            inout(reg) ptr => result,
            in(reg) modifier,
            options(nostack)
        )
    };
    result
}

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
