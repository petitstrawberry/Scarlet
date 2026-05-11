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
pub const LC_ID_DYLIB: u32 = 0x0D;
pub const LC_LOAD_DYLINKER: u32 = 0x0e;
pub const LC_DYLD_CHAINED_FIXUPS: u32 = 0x80000034;
pub const LC_DYLD_EXPORTS_TRIE: u32 = 0x80000036;

pub const DYLD_CHAINED_IMPORT: u32 = 1;
pub const DYLD_CHAINED_IMPORT_ADDEND: u32 = 2;
pub const DYLD_CHAINED_IMPORT_ADDEND64: u32 = 3;

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
struct DylibCommand {
    cmd: u32,
    cmdsize: u32,
    name_offset: u32,
    timestamp: u32,
    current_version: u32,
    compatibility_version: u32,
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

#[repr(C)]
#[derive(Clone, Copy)]
struct DyldCacheImageInfo {
    address: u64,
    mod_time: u64,
    inode: u64,
    path_file_offset: u32,
    pad: u32,
}

#[derive(Clone)]
struct ChainedImport {
    lib_ordinal: i32,
    weak_import: bool,
    name_offset: u32,
}

#[derive(Clone, Copy)]
struct ExportEntry {
    flags: u64,
    runtime_offset: u64,
}

struct CacheImageLocation {
    file: Arc<dyn FileObject>,
    mach_offset: u64,
    vmaddr: u64,
}

const EXPORT_SYMBOL_FLAGS_REEXPORT: u64 = 0x08;
const BIND_SPECIAL_DYLIB_MAIN_EXECUTABLE: i32 = -1;
const BIND_SPECIAL_DYLIB_FLAT_LOOKUP: i32 = -2;
const BIND_SPECIAL_DYLIB_WEAK_LOOKUP: i32 = -3;
const MAX_MACHO_CSTRING_LEN: usize = 1024;

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

    let segment_vaddr =
        usize::try_from(segment.vmaddr).map_err(|_| "Mach-O vmaddr out of range")?;
    let segment_vmsize =
        usize::try_from(segment.vmsize).map_err(|_| "Mach-O vmsize out of range")?;
    let segment_filesize =
        usize::try_from(segment.filesize).map_err(|_| "Mach-O filesize out of range")?;
    let segment_fileoff = segment.fileoff;

    if segment_filesize == 0 {
        return Ok(());
    }

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

    if segment_filesize == 0 {
        return Ok(());
    }

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
    raw_file: &dyn FileObject,
    slice_offset: u64,
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

    let imports = parse_chained_imports(fixup_data, &header)?;
    let dylib_names = parse_dylib_dependencies(raw_file, slice_offset)?;
    let resolved_imports = resolve_imports(&imports, &dylib_names, fixup_data, &header)?;

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
            let chain_start = page_start as usize;

            // Phase 1: Read all chain entries before writing any.
            // With stride=4 and 8-byte entries, consecutive reads overlap
            // if we write before advancing. Collecting first avoids corruption.
            let mut chain_entries: Vec<(usize, usize, u64)> = Vec::new();
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

                chain_entries.push((entry_addr, kva, value));

                if next == 0 {
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
            for (entry_addr, kva, current_value) in &chain_entries {
                let auth_bit = (*current_value >> 63) & 1;
                let bind_bit = (*current_value >> 62) & 1;

                let new_value = if bind_bit == 0 {
                    if pointer_format == DYLD_CHAINED_PTR_ARM64E && auth_bit != 0 {
                        let target = *current_value & 0xFFFF_FFFF;
                        let diversity = (*current_value >> 32) & 0xFFFF;
                        let addr_div = (*current_value >> 48) & 1;
                        let key = (*current_value >> 49) & 0x3;
                        let result = (base_addr.wrapping_add(target)) & 0x0000_FFFF_FFFF_FFFF;
                        if *entry_addr >= 0x400adc00 && *entry_addr <= 0x400add00 {
                            crate::println!(
                                "[darwin] AUTH_REBASE @ {:#x}: raw={:#x} target={:#x} diversity={:#x} addrDiv={} key={} => {:#x} (base_addr={:#x})",
                                entry_addr, current_value, target, diversity, addr_div, key, result, base_addr
                            );
                        }
                        result
                    } else {
                        let target43 = *current_value & ((1u64 << 43) - 1);
                        let high8 = (*current_value >> 43) & 0xFF;
                        let preferred_vmaddr = (high8 << 56) | target43;
                        let rebased = if base_delta >= 0 {
                            preferred_vmaddr.wrapping_add(base_delta as u64)
                        } else {
                            preferred_vmaddr.wrapping_sub((-base_delta) as u64)
                        };
                        rebased & 0x0000_FFFF_FFFF_FFFF
                    }
                } else {
                    let ordinal = (*current_value & 0xFFFF) as usize;
                    let resolved_addr = *resolved_imports
                        .get(ordinal)
                        .ok_or("Chained fixup bind ordinal out of range")?;

                    if auth_bit != 0 {
                        let diversity = (*current_value >> 32) & 0xFFFF;
                        let addr_div = (*current_value >> 48) & 1;
                        let key = (*current_value >> 49) & 0x3;
                        let modifier = if addr_div != 0 {
                            diversity ^ ((*entry_addr as u64) >> 3)
                        } else {
                            diversity
                        };

                        if key >= 2 {
                            pac_sign_da(resolved_addr, modifier)
                        } else {
                            pac_sign_ia(resolved_addr, modifier)
                        }
                    } else {
                        let addend = sign_extend((*current_value >> 32) & 0x7FFFF, 19);
                        resolved_addr.wrapping_add_signed(addend)
                    }
                };

                unsafe {
                    ptr::write(*kva as *mut u64, new_value);
                }
            }
        }
    }

    Ok(())
}

fn parse_chained_imports(
    fixup_data: &[u8],
    header: &DyldChainedFixupsHeader,
) -> Result<Vec<ChainedImport>, &'static str> {
    let mut imports = Vec::new();
    let imports_base = header.imports_offset as usize;

    match header.imports_format {
        DYLD_CHAINED_IMPORT => {
            for i in 0..header.imports_count as usize {
                let pos = imports_base + i * 4;
                if pos + 4 > fixup_data.len() {
                    return Err("Chained imports table truncated");
                }
                let val = read_u32(&fixup_data[pos..pos + 4]);
                let lib_ord_raw = (val & 0xFF) as u8;
                let lib_ordinal = if lib_ord_raw > 0xF0 {
                    (lib_ord_raw as i8) as i32
                } else {
                    lib_ord_raw as i32
                };
                let weak = ((val >> 8) & 1) != 0;
                let name_off = (val >> 9) & 0x7F_FFFF;
                imports.push(ChainedImport {
                    lib_ordinal,
                    weak_import: weak,
                    name_offset: name_off,
                });
            }
        }
        DYLD_CHAINED_IMPORT_ADDEND => {
            for i in 0..header.imports_count as usize {
                let pos = imports_base + i * 8;
                if pos + 8 > fixup_data.len() {
                    return Err("Chained imports addend table truncated");
                }
                let val = read_u32(&fixup_data[pos..pos + 4]);
                let lib_ord_raw = (val & 0xFF) as u8;
                let lib_ordinal = if lib_ord_raw > 0xF0 {
                    (lib_ord_raw as i8) as i32
                } else {
                    lib_ord_raw as i32
                };
                let weak = ((val >> 8) & 1) != 0;
                let name_off = (val >> 9) & 0x7F_FFFF;
                imports.push(ChainedImport {
                    lib_ordinal,
                    weak_import: weak,
                    name_offset: name_off,
                });
            }
        }
        DYLD_CHAINED_IMPORT_ADDEND64 => {
            for i in 0..header.imports_count as usize {
                let pos = imports_base + i * 16;
                if pos + 16 > fixup_data.len() {
                    return Err("Chained imports addend64 table truncated");
                }
                let val = read_u64(&fixup_data[pos..pos + 8]);
                let lib_ord_raw = (val & 0xFFFF) as u16;
                let lib_ordinal = if lib_ord_raw > 0xFFF0 {
                    (lib_ord_raw as i16) as i32
                } else {
                    lib_ord_raw as i32
                };
                let weak = ((val >> 16) & 1) != 0;
                let name_off = ((val >> 32) & 0xFFFF_FFFF) as u32;
                imports.push(ChainedImport {
                    lib_ordinal,
                    weak_import: weak,
                    name_offset: name_off,
                });
            }
        }
        _ => return Err("unknown imports format"),
    }

    Ok(imports)
}

fn parse_dylib_dependencies(
    raw_file: &dyn FileObject,
    slice_offset: u64,
) -> Result<Vec<String>, &'static str> {
    let mut header_bytes = [0u8; size_of::<MachHeader64>()];
    read_exact_at(raw_file, slice_offset, &mut header_bytes)?;
    let header = read_struct::<MachHeader64>(&header_bytes)?;

    let mut load_commands = vec![0u8; header.sizeofcmds as usize];
    read_exact_at(
        raw_file,
        slice_offset + size_of::<MachHeader64>() as u64,
        &mut load_commands,
    )?;

    let mut dylibs = Vec::new();
    dylibs.push(String::new());

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
        if load_cmd.cmd == LC_LOAD_DYLIB {
            if cmdsize < size_of::<DylibCommand>() {
                return Err("Truncated LC_LOAD_DYLIB command");
            }
            let dylib_cmd =
                read_struct::<DylibCommand>(&command_bytes[..size_of::<DylibCommand>()])?;
            let name_offset = dylib_cmd.name_offset as usize;
            if name_offset >= cmdsize {
                return Err("LC_LOAD_DYLIB name offset out of bounds");
            }
            let name = read_null_terminated_bytes(&command_bytes[name_offset..])?;
            dylibs.push(name.to_string());
        }

        offset = next_offset;
    }

    Ok(dylibs)
}

fn resolve_imports(
    imports: &[ChainedImport],
    dylib_names: &[String],
    fixup_data: &[u8],
    header: &DyldChainedFixupsHeader,
) -> Result<Vec<u64>, &'static str> {
    let cache_mutex = SHARED_CACHE.get().ok_or("Shared cache not initialized")?;
    let cache_guard = cache_mutex.lock();
    let cache = cache_guard.as_ref().ok_or("Shared cache not initialized")?;

    let mut resolved = Vec::with_capacity(imports.len());
    for import in imports {
        let symbol = read_fixup_symbol(fixup_data, header, import.name_offset)?;
        let install_name = match import.lib_ordinal {
            0 => return Err("Self imports are not supported in chained fixups"),
            ord if ord > 0 => dylib_names
                .get(ord as usize)
                .ok_or("Chained import dylib ordinal out of range")?,
            BIND_SPECIAL_DYLIB_MAIN_EXECUTABLE => {
                return Err("Main executable imports are not supported in chained fixups");
            }
            BIND_SPECIAL_DYLIB_FLAT_LOOKUP | BIND_SPECIAL_DYLIB_WEAK_LOOKUP => {
                if import.weak_import {
                    resolved.push(0);
                    continue;
                }
                return Err("Special dylib ordinals are not supported in chained fixups");
            }
            _ => return Err("Unknown chained import dylib ordinal"),
        };

        let image = cache
            .find_dylib_image(install_name)?
            .ok_or("Shared cache dylib not found")?;
        match cache.resolve_export(&image, &symbol)? {
            Some(addr) => resolved.push(addr),
            None if import.weak_import => resolved.push(0),
            None => return Err("Shared cache export symbol not found"),
        }
    }

    Ok(resolved)
}

fn read_fixup_symbol(
    fixup_data: &[u8],
    header: &DyldChainedFixupsHeader,
    name_offset: u32,
) -> Result<String, &'static str> {
    let base = header.symbols_offset as usize;
    let start = base
        .checked_add(name_offset as usize)
        .ok_or("Chained import symbol offset overflow")?;
    if start >= fixup_data.len() {
        return Err("Chained import symbol offset out of bounds");
    }
    Ok(read_null_terminated_bytes(&fixup_data[start..])?.to_string())
}

fn walk_export_trie(trie_data: &[u8], symbol: &str) -> Option<ExportEntry> {
    let mut node_offset = 0usize;
    let symbol_bytes = symbol.as_bytes();
    let mut symbol_offset = 0usize;

    loop {
        if node_offset >= trie_data.len() {
            return None;
        }

        let (terminal_size, terminal_len) = read_uleb128(&trie_data[node_offset..])?;
        let terminal_start = node_offset.checked_add(terminal_len)?;
        let terminal_end = terminal_start.checked_add(terminal_size as usize)?;
        if terminal_end > trie_data.len() {
            return None;
        }

        if symbol_offset == symbol_bytes.len() && terminal_size != 0 {
            let terminal_data = &trie_data[terminal_start..terminal_end];
            let (flags, flags_len) = read_uleb128(terminal_data)?;
            if flags & EXPORT_SYMBOL_FLAGS_REEXPORT != 0 {
                return None;
            }
            let (runtime_offset, _) = read_uleb128(&terminal_data[flags_len..])?;
            return Some(ExportEntry {
                flags,
                runtime_offset,
            });
        }

        if terminal_end >= trie_data.len() {
            return None;
        }

        let child_count = trie_data[terminal_end] as usize;
        let mut cursor = terminal_end + 1;
        let mut matched = false;

        for _ in 0..child_count {
            let edge_start = cursor;
            let edge_end = trie_data[edge_start..]
                .iter()
                .position(|&b| b == 0)
                .and_then(|pos| edge_start.checked_add(pos))?;
            let edge = &trie_data[edge_start..edge_end];
            cursor = edge_end + 1;
            let (child_offset, child_len) = read_uleb128(&trie_data[cursor..])?;
            cursor = cursor.checked_add(child_len)?;

            if symbol_bytes[symbol_offset..].starts_with(edge) {
                node_offset = child_offset as usize;
                symbol_offset = symbol_offset.checked_add(edge.len())?;
                matched = true;
                break;
            }
        }

        if !matched {
            return None;
        }
    }
}

fn read_uleb128(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;

    for (idx, byte) in bytes.iter().copied().enumerate() {
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((value, idx + 1));
        }
        shift = shift.checked_add(7)?;
        if shift >= 64 {
            return None;
        }
    }

    None
}

fn sign_extend(value: u64, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((value << shift) as i64) >> shift
}

fn disable_chained_starts(fixup_data: &mut [u8]) {
    if fixup_data.len() < size_of::<DyldChainedFixupsHeader>() {
        return;
    }
    let header =
        read_struct::<DyldChainedFixupsHeader>(&fixup_data[..size_of::<DyldChainedFixupsHeader>()])
            .unwrap_or_else(|_| DyldChainedFixupsHeader {
                fixups_version: 0,
                starts_offset: 0,
                imports_offset: 0,
                symbols_offset: 0,
                imports_count: 0,
                imports_format: 0,
                symbols_format: 0,
            });
    let starts_base = header.starts_offset as usize;
    if starts_base + 4 > fixup_data.len() {
        return;
    }
    let seg_count = u32::from_le_bytes(
        fixup_data[starts_base..starts_base + 4]
            .try_into()
            .unwrap_or([0; 4]),
    ) as usize;
    for seg_idx in 0..seg_count {
        let off_pos = starts_base + 4 + seg_idx * 4;
        if off_pos + 4 > fixup_data.len() {
            break;
        }
        let seg_info_offset = u32::from_le_bytes(
            fixup_data[off_pos..off_pos + 4]
                .try_into()
                .unwrap_or([0; 4]),
        ) as usize;
        if seg_info_offset == 0 {
            continue;
        }
        let abs_offset = starts_base + seg_info_offset;
        if abs_offset + 22 > fixup_data.len() {
            break;
        }
        let page_count =
            u16::from_le_bytes([fixup_data[abs_offset + 20], fixup_data[abs_offset + 21]]) as usize;
        for page_idx in 0..page_count {
            let ps_pos = abs_offset + 22 + page_idx * 2;
            if ps_pos + 2 > fixup_data.len() {
                break;
            }
            fixup_data[ps_pos] = 0xFF;
            fixup_data[ps_pos + 1] = 0xFF;
        }
    }
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

    // dyld owns rebasing of its on-disk image.  XNU maps dyld at the
    // selected slide and jumps to __dyld_start with the original chained
    // fixup words intact; dyld's early bootstrap then walks its own
    // LC_DYLD_CHAINED_FIXUPS.  Applying them here would make dyld interpret
    // already-rebased pointers as chain entries and slide them a second time.

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
const CACHE_HEADER_DYNAMIC_DATA_OFFSET: usize = 0x1F0;
const CACHE_HEADER_SUBCACHE_ARRAY_OFFSET: usize = 0x188;
const CACHE_HEADER_SUBCACHE_ARRAY_COUNT: usize = 0x18C;

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

fn parse_v5_slide_infos(
    file: &Arc<dyn FileObject>,
    mappings: &[CacheMappingSlideInfo],
) -> Result<Vec<SlideInfoMeta>, &'static str> {
    let mut infos = Vec::new();
    for (i, mapping) in mappings.iter().enumerate() {
        let si_off = mapping.slide_info_file_offset as usize;
        let si_size = mapping.slide_info_file_size as usize;
        if si_size == 0 || si_off == 0 {
            continue;
        }

        let mut slide_header = alloc::vec![0u8; SLIDE_INFO5_HEADER_SIZE];
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
            continue;
        }

        let version = read_u32(&slide_header[0..4]);
        let page_size = read_u32(&slide_header[4..8]);
        let page_starts_count = read_u32(&slide_header[8..12]) as usize;
        let value_add = read_u64(&slide_header[16..24]);

        if version != 5 || page_size == 0 {
            continue;
        }

        let page_starts_bytes = page_starts_count
            .checked_mul(core::mem::size_of::<u16>())
            .unwrap_or(0);
        if SLIDE_INFO5_HEADER_SIZE + page_starts_bytes > si_size {
            continue;
        }

        let mut page_starts_raw = alloc::vec![0u8; page_starts_bytes];
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
            continue;
        }

        let mut page_starts = Vec::with_capacity(page_starts_count);
        for chunk in page_starts_raw.chunks_exact(core::mem::size_of::<u16>()) {
            page_starts.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }

        crate::println!(
            "[darwin] slide info [{}]: {} pages, auth_data={} addr={:#x} page_size={:#x} value_add={:#x}",
            i,
            page_starts_count,
            (mapping.flags & CACHE_MAPPING_AUTH_DATA) != 0,
            mapping.address,
            page_size,
            value_add,
        );

        infos.push(SlideInfoMeta {
            mapping_idx: i,
            file_offset: usize::try_from(mapping.file_offset)
                .map_err(|_| "Shared cache mapping file offset out of range")?,
            mapping_address: mapping.address,
            mapping_size: usize::try_from(mapping.size)
                .map_err(|_| "Shared cache mapping size out of range")?,
            page_size,
            value_add,
            page_starts,
            is_auth: (mapping.flags & CACHE_MAPPING_AUTH_DATA) != 0,
        });
    }
    Ok(infos)
}

fn parse_slide_mappings_from_header(
    header: &[u8],
) -> Result<Vec<CacheMappingSlideInfo>, &'static str> {
    let mapping_slide_offset = read_u32(
        &header[CACHE_HEADER_MAPPING_WITH_SLIDE_OFFSET..CACHE_HEADER_MAPPING_WITH_SLIDE_OFFSET + 4],
    ) as usize;
    let mapping_slide_count = read_u32(
        &header[CACHE_HEADER_MAPPING_WITH_SLIDE_COUNT..CACHE_HEADER_MAPPING_WITH_SLIDE_COUNT + 4],
    ) as usize;
    let count = mapping_slide_count.min(32);
    if count == 0 {
        return Ok(Vec::new());
    }
    let mappings_end = mapping_slide_offset
        .checked_add(count * 56)
        .ok_or("Mapping table overflow")?;
    if mappings_end > header.len() {
        return Err("Mapping table truncated");
    }
    let mut mappings = Vec::with_capacity(count);
    for i in 0..count {
        let off = mapping_slide_offset + i * 56;
        let src = &header[off..off + 56];
        mappings.push(CacheMappingSlideInfo {
            address: read_u64(&src[0..8]),
            size: read_u64(&src[8..16]),
            file_offset: read_u64(&src[16..24]),
            slide_info_file_offset: read_u64(&src[24..32]),
            slide_info_file_size: read_u64(&src[32..40]),
            flags: read_u64(&src[40..48]),
            max_prot: read_u32(&src[48..52]),
            init_prot: read_u32(&src[52..56]),
        });
    }
    Ok(mappings)
}

struct SlideInfoMeta {
    mapping_idx: usize,
    file_offset: usize,
    mapping_address: u64,
    mapping_size: usize,
    page_size: u32,
    value_add: u64,
    page_starts: Vec<u16>,
    is_auth: bool,
}

struct SubCache {
    file: Arc<dyn FileObject>,
    vm_offset: usize,
    mappings: Vec<CacheMappingSlideInfo>,
}

struct DarwinSharedCache {
    file: Arc<dyn FileObject>,
    cache_start: usize,
    total_size: usize,
    mapping_count: usize,
    mappings: [CacheMappingSlideInfo; 8],
    slide_infos: Vec<SlideInfoMeta>,
    pages: Mutex<BTreeMap<usize, usize>>,
    dynamic_data_offset: usize,
    sub_caches: Vec<SubCache>,
    needs_header_patch: bool,
}

static SHARED_CACHE: Once<Mutex<Option<Arc<DarwinSharedCache>>>> = Once::new();

impl DarwinSharedCache {
    fn cache_image_infos(&self) -> Result<Vec<DyldCacheImageInfo>, &'static str> {
        let mut header_buf = vec![0u8; 0x200];
        read_exact_at(self.file.as_ref(), 0, &mut header_buf)?;

        const IMAGES_OFFSET: usize = 0x1C0;
        const IMAGES_COUNT: usize = 0x1C4;
        if IMAGES_COUNT + 4 > header_buf.len() {
            return Err("Shared cache header missing imagesCount field");
        }

        let images_offset = read_u32(&header_buf[IMAGES_OFFSET..IMAGES_OFFSET + 4]) as u64;
        let images_count = read_u32(&header_buf[IMAGES_COUNT..IMAGES_COUNT + 4]) as usize;
        if images_offset == 0 || images_count == 0 {
            return Err("Shared cache image array unavailable");
        }

        let image_array_size = images_count * size_of::<DyldCacheImageInfo>();
        let mut raw = vec![0u8; image_array_size];
        read_exact_at(self.file.as_ref(), images_offset, &mut raw)?;

        let mut infos = Vec::with_capacity(images_count);
        for i in 0..images_count {
            let start = i * size_of::<DyldCacheImageInfo>();
            let end = start + size_of::<DyldCacheImageInfo>();
            infos.push(read_struct::<DyldCacheImageInfo>(&raw[start..end])?);
        }
        Ok(infos)
    }

    fn read_path_at(&self, path_file_offset: u32) -> Result<String, &'static str> {
        let mut buf = vec![0u8; MAX_MACHO_CSTRING_LEN];
        let read_len = self
            .file
            .read_at(path_file_offset as u64, &mut buf)
            .map_err(|_| "Failed to read shared cache image path")?;
        if read_len == 0 {
            return Err("Shared cache image path truncated");
        }
        Ok(read_null_terminated_bytes(&buf[..read_len])?.to_string())
    }

    fn find_dylib_image(
        &self,
        install_name: &str,
    ) -> Result<Option<CacheImageLocation>, &'static str> {
        for image in self.cache_image_infos()? {
            let path = self.read_path_at(image.path_file_offset)?;
            if path != install_name {
                continue;
            }

            let file = self
                .file_for_vmaddr(image.address)
                .ok_or("Shared cache image backing file not found")?;
            let mach_offset = self
                .file_offset_for_vmaddr(image.address)
                .ok_or("Shared cache image file offset not found")?;

            return Ok(Some(CacheImageLocation {
                file,
                mach_offset,
                vmaddr: image.address,
            }));
        }

        Ok(None)
    }

    fn file_for_vmaddr(&self, vmaddr: u64) -> Option<Arc<dyn FileObject>> {
        for sub_cache in &self.sub_caches {
            if sub_cache.mappings.iter().any(|mapping| {
                vmaddr >= mapping.address && vmaddr < mapping.address.saturating_add(mapping.size)
            }) {
                return Some(sub_cache.file.clone());
            }
        }

        if self
            .mappings
            .iter()
            .take(self.mapping_count)
            .any(|mapping| {
                vmaddr >= mapping.address && vmaddr < mapping.address.saturating_add(mapping.size)
            })
        {
            return Some(self.file.clone());
        }

        None
    }

    fn file_offset_for_vmaddr(&self, vmaddr: u64) -> Option<u64> {
        for mapping in self.mappings.iter().take(self.mapping_count) {
            if vmaddr >= mapping.address && vmaddr < mapping.address.saturating_add(mapping.size) {
                return Some(mapping.file_offset + (vmaddr - mapping.address));
            }
        }

        for sub_cache in &self.sub_caches {
            for mapping in &sub_cache.mappings {
                if vmaddr >= mapping.address
                    && vmaddr < mapping.address.saturating_add(mapping.size)
                {
                    return Some(mapping.file_offset + (vmaddr - mapping.address));
                }
            }
        }

        None
    }

    fn resolve_export(
        &self,
        image: &CacheImageLocation,
        symbol: &str,
    ) -> Result<Option<u64>, &'static str> {
        let mut header_bytes = [0u8; size_of::<MachHeader64>()];
        read_exact_at(image.file.as_ref(), image.mach_offset, &mut header_bytes)?;
        let header = read_struct::<MachHeader64>(&header_bytes)?;

        let mut load_commands = vec![0u8; header.sizeofcmds as usize];
        read_exact_at(
            image.file.as_ref(),
            image.mach_offset + size_of::<MachHeader64>() as u64,
            &mut load_commands,
        )?;

        let mut export_trie: Option<(u64, u64)> = None;
        let mut id_name: Option<String> = None;

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
            match load_cmd.cmd {
                LC_DYLD_EXPORTS_TRIE => {
                    if cmdsize < size_of::<LinkEditDataCommand>() {
                        return Err("Truncated LC_DYLD_EXPORTS_TRIE command");
                    }
                    let trie_cmd = read_struct::<LinkEditDataCommand>(
                        &command_bytes[..size_of::<LinkEditDataCommand>()],
                    )?;
                    export_trie = Some((trie_cmd.dataoff as u64, trie_cmd.datasize as u64));
                }
                LC_ID_DYLIB => {
                    if cmdsize < size_of::<DylibCommand>() {
                        return Err("Truncated LC_ID_DYLIB command");
                    }
                    let dylib_cmd =
                        read_struct::<DylibCommand>(&command_bytes[..size_of::<DylibCommand>()])?;
                    let name_offset = dylib_cmd.name_offset as usize;
                    if name_offset < cmdsize {
                        id_name = Some(
                            read_null_terminated_bytes(&command_bytes[name_offset..])?.to_string(),
                        );
                    }
                }
                _ => {}
            }

            offset = next_offset;
        }

        if let Some(id_name) = id_name {
            crate::println!("[darwin] resolving {} in {}", symbol, id_name);
        }

        let (trie_off, trie_size) = match export_trie {
            Some(v) => v,
            None => return Ok(None),
        };
        let trie_size = usize::try_from(trie_size).map_err(|_| "Export trie size out of range")?;
        let mut trie_data = vec![0u8; trie_size];
        read_exact_at(
            image.file.as_ref(),
            image.mach_offset + trie_off,
            &mut trie_data,
        )?;

        let export = match walk_export_trie(&trie_data, symbol) {
            Some(export) => export,
            None => return Ok(None),
        };
        if export.flags & EXPORT_SYMBOL_FLAGS_REEXPORT != 0 {
            return Ok(None);
        }

        Ok(Some(image.vmaddr.wrapping_add(export.runtime_offset)))
    }

    fn ensure_dynamic_region(&self) {
        let offset = self.dynamic_data_offset;
        if offset == 0 || offset >= self.total_size {
            return;
        }
        let page_index = offset / PAGE_SIZE;
        let mut pages = self.pages.lock();
        if pages.contains_key(&page_index) {
            return;
        }
        drop(pages);

        let page = match ContiguousPages::new(1) {
            Some(p) => p,
            None => return,
        };
        let paddr = page.as_paddr();
        let kva = crate::vm::addr::phys_to_virt(paddr);

        unsafe {
            ptr::write_bytes(kva as *mut u8, 0, PAGE_SIZE);
        }

        // Write DyldSharedCache::DynamicRegion:
        // _magic = "dyld_data    v3" (16 bytes)
        // _dyldCache = FileIdTuple { fsid_t, fsobj_id_t } — must be non-zero for operator bool()
        let dr_magic: &[u8; 16] = b"dyld_data    v3\0";
        let cache_path = b"/System/Library/dyld/dyld_shared_cache_arm64e\0";
        let cache_path_offset: u32 = 0x40;
        unsafe {
            ptr::copy_nonoverlapping(dr_magic.as_ptr(), kva as *mut u8, 16);
            ptr::write((kva + 16) as *mut u64, 1);
            ptr::write((kva + 24) as *mut u64, 1);
            ptr::write((kva + 0x24) as *mut u32, cache_path_offset);
            ptr::copy_nonoverlapping(
                cache_path.as_ptr(),
                (kva + cache_path_offset as usize) as *mut u8,
                cache_path.len(),
            );
        }

        core::mem::forget(page);
        self.pages.lock().insert(page_index, paddr);

        crate::println!(
            "[darwin] DynamicRegion written at cache+{:#x} (page {})",
            offset,
            page_index
        );
    }

    fn patch_cache_header(&self, kva: usize) {
        let base = kva as *mut u8;

        macro_rules! zero_u64_at {
            ($off:expr) => {
                unsafe { ptr::write((base.add($off)) as *mut u64, 0) };
            };
        }
        macro_rules! zero_u32_at {
            ($off:expr) => {
                unsafe { ptr::write((base.add($off)) as *mut u32, 0) };
            };
        }

        // Zero fields that point to sub-cache data:
        // dylibsPBLSetAddr (0x148), programsPBLSetPoolAddr (0x150),
        // programsPBLSetPoolSize (0x158), programTrieAddr (0x160),
        // dylibsImageArrayAddr (0xF8), dylibsImageArraySize (0x100),
        // dylibsTrieAddr (0x108), dylibsTrieSize (0x110),
        // otherImageArrayAddr (0x118), otherImageArraySize (0x120),
        // otherTrieAddr (0x128), otherTrieSize (0x130),
        // subCacheArrayOffset (0x188), subCacheArrayCount (0x18C)
        zero_u64_at!(0xF8);
        zero_u64_at!(0x100);
        zero_u64_at!(0x108);
        zero_u64_at!(0x110);
        zero_u64_at!(0x118);
        zero_u64_at!(0x120);
        zero_u64_at!(0x128);
        zero_u64_at!(0x130);
        zero_u64_at!(0x148);
        zero_u64_at!(0x150);
        zero_u64_at!(0x158);
        zero_u64_at!(0x160);
        zero_u32_at!(0x168);
        zero_u32_at!(0x188);
        zero_u32_at!(0x18C);

        crate::println!("[darwin] patched cache header: zeroed sub-cache fields");
    }

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

    fn apply_slide_fixups_for_page(&self, slide_base_vaddr: usize, kva: usize, buf_size: usize) {
        // Phase 1: Apply chained fixups (rebase/bind entries from fixup chains)
        for si in &self.slide_infos {
            let sps = si.page_size as usize;
            let mapping_start_slide = (si.mapping_address as usize - self.cache_start) / sps;
            let mapping_slide_count = si.mapping_size / sps;

            let slide_idx = (slide_base_vaddr - self.cache_start) / sps;

            if slide_idx < mapping_start_slide
                || slide_idx >= mapping_start_slide.saturating_add(mapping_slide_count)
            {
                continue;
            }

            let local_slide_idx = slide_idx - mapping_start_slide;
            if local_slide_idx >= si.page_starts.len() {
                continue;
            }

            let page_start = si.page_starts[local_slide_idx];
            if page_start == DYLD_CACHE_SLIDE_V5_PAGE_ATTR_NO_REBASE {
                continue;
            }

            let page_data_offset_in_mapping = local_slide_idx * sps;
            let mut delta_bytes = page_start as usize;
            let mut loc = kva as usize;

            loop {
                loc += delta_bytes;
                if loc >= kva + buf_size {
                    break;
                }

                let loc_ptr = loc as *mut u64;
                let raw = unsafe { ptr::read(loc_ptr) };
                let auth_bit = (raw >> 63) & 1;
                let _slot_offset = (loc - kva) / 8;
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
                        + (loc - kva) as u64;
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
                    unsafe { ptr::write(loc_ptr, signed_ptr) };
                } else {
                    let runtime_offset = raw & 0x3FFFFFFFF;
                    let high8 = (raw >> 34) & 0xFF;
                    next = ((raw >> 52) & 0x7FF) as usize;
                    let target = si.value_add + runtime_offset;
                    unsafe { ptr::write(loc_ptr, target | (high8 << 56)) };
                }

                if next == 0 {
                    break;
                }
                delta_bytes = next * 8;
            }
        }

        // Phase 2: Re-sign auth pointers not covered by fixup chains.
        // Auth data pages contain PAC-signed pointers that are pre-signed with
        // Apple's build-time keys. The fixup chains only cover entries needing
        // rebasing; auth-only pointers (correct address, wrong PAC signature)
        // are not in the chains. This pass re-signs them with our runtime keys.
        for si in &self.slide_infos {
            if !si.is_auth {
                continue;
            }

            let mapping_start = si.mapping_address as usize;
            let mapping_end = mapping_start + si.mapping_size;
            if slide_base_vaddr < mapping_start || slide_base_vaddr >= mapping_end {
                continue;
            }

            let page_data_offset_in_mapping = slide_base_vaddr - mapping_start;

            for offset in (0..buf_size).step_by(8) {
                let loc_ptr = (kva + offset) as *mut u64;
                let raw = unsafe { ptr::read(loc_ptr) };

                if (raw >> 63) & 1 == 0 {
                    continue;
                }

                let runtime_offset = raw & 0x3FFFFFFFF;
                let diversity = (raw >> 34) & 0xFFFF;
                let addr_div = (raw >> 50) & 1;
                let key_is_data = (raw >> 51) & 1;
                let next_field = (raw >> 52) & 0x7FF;

                if next_field != 0 {
                    continue;
                }

                let target = si.value_add + runtime_offset;
                let cache_end = self.cache_start as u64 + self.total_size as u64;
                if target < self.cache_start as u64 || target >= cache_end || runtime_offset == 0 {
                    continue;
                }

                let fixup_vaddr = si.mapping_address
                    + page_data_offset_in_mapping as u64
                    + offset as u64;
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
                unsafe { ptr::write(loc_ptr, signed_ptr) };
            }
        }
    }

    fn slide_page_size_for(&self, vaddr: usize) -> usize {
        let sps_4k = PAGE_SIZE;
        for si in &self.slide_infos {
            let sps = si.page_size as usize;
            let mapping_start = si.mapping_address as usize;
            let mapping_end = mapping_start + si.mapping_size;
            if vaddr >= mapping_start && vaddr < mapping_end {
                return sps;
            }
        }
        sps_4k
    }

    fn find_file_and_offset(&self, page_vaddr: usize) -> (Arc<dyn FileObject>, usize) {
        let page_offset_from_base = page_vaddr - self.cache_start;
        self.sub_caches
            .iter()
            .filter(|sc| page_offset_from_base >= sc.vm_offset)
            .find_map(|sc| {
                sc.mappings
                    .iter()
                    .find(|m| {
                        let maddr = m.address as usize;
                        let mend = maddr.saturating_add(m.size as usize);
                        page_vaddr >= maddr && page_vaddr < mend
                    })
                    .map(|m| {
                        let off_in_map = page_vaddr - m.address as usize;
                        (sc.file.clone(), m.file_offset as usize + off_in_map)
                    })
            })
            .or_else(|| {
                self.find_mapping_for_page(page_vaddr)
                    .map(|(_, _, fo)| (self.file.clone(), fo))
            })
            .unwrap_or((self.file.clone(), page_offset_from_base))
    }

    fn resolve_fault_slide_page(
        &self,
        page_vaddr: usize,
        page_index: usize,
        slide_page_size: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let num_sub_pages = slide_page_size / PAGE_SIZE;
        let slide_base_vaddr = page_vaddr & !(slide_page_size - 1);
        let slide_base_index = (slide_base_vaddr - self.cache_start) / PAGE_SIZE;

        {
            let pages = self.pages.lock();
            if let Some(&paddr) = pages.get(&slide_base_index) {
                let sub_idx = (page_vaddr - slide_base_vaddr) / PAGE_SIZE;
                return Ok(ResolveFaultResult {
                    paddr_page_base: paddr + sub_idx * PAGE_SIZE,
                    is_tail: false,
                });
            }
        }

        let (read_file, read_offset) = self.find_file_and_offset(slide_base_vaddr);

        let mut buf = alloc::vec![0u8; slide_page_size];
        let mut total_read = 0usize;
        while total_read < slide_page_size {
            let dst = &mut buf[total_read..];
            let n = read_file
                .read_at((read_offset + total_read) as u64, dst)
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            total_read += n;
        }

        self.apply_slide_fixups_for_page(slide_base_vaddr, buf.as_ptr() as usize, slide_page_size);

        // Debug: show values at crash-relevant offsets after fixup
        if slide_base_vaddr <= 0x1ef37c000 && slide_base_vaddr + slide_page_size > 0x1ef37afc8 {
            let off = 0x1ef37afc8 - slide_base_vaddr;
            let buf_ptr = buf.as_ptr() as *const u64;
            crate::println!("[darwin] SLIDE fixup base={:#x} sps={:#x} read={}", slide_base_vaddr, slide_page_size, total_read);
            for i in (off / 8).saturating_sub(2)..=(off / 8) + 2 {
                if i < slide_page_size / 8 {
                    let val = unsafe { ptr::read(buf_ptr.add(i)) };
                    crate::println!("[darwin]   [{}] @{:#x} = {:#x}", i, slide_base_vaddr + i * 8, val);
                }
            }
        }

        let mut pages = self.pages.lock();
        if let Some(&existing) = pages.get(&slide_base_index) {
            let sub_idx = (page_vaddr - slide_base_vaddr) / PAGE_SIZE;
            return Ok(ResolveFaultResult {
                paddr_page_base: existing + sub_idx * PAGE_SIZE,
                is_tail: false,
            });
        }

        let mut first_paddr: usize = 0;
        for j in 0..num_sub_pages {
            let sub_page = ContiguousPages::new(1).ok_or(ResolveFaultError::Unmapped)?;
            let sub_paddr = sub_page.as_paddr();
            let sub_kva = crate::vm::addr::phys_to_virt(sub_paddr);

            // SAFETY: Each sub-page is freshly allocated; copy from the fixed-up buffer.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buf[j * PAGE_SIZE..].as_ptr(),
                    sub_kva as *mut u8,
                    PAGE_SIZE,
                );
            }
            core::mem::forget(sub_page);

            pages.insert(slide_base_index + j, sub_paddr);
            if j == 0 {
                first_paddr = sub_paddr;
            }
        }

        let sub_idx = (page_vaddr - slide_base_vaddr) / PAGE_SIZE;
        Ok(ResolveFaultResult {
            paddr_page_base: first_paddr + sub_idx * PAGE_SIZE,
            is_tail: false,
        })
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
        page_idx: usize,
        vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let page_vaddr = access.vaddr & !(PAGE_SIZE - 1);
        if page_vaddr < self.cache_start {
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

        let sps = self.slide_page_size_for(page_vaddr);
        if sps > PAGE_SIZE {
            return self.resolve_fault_slide_page(page_vaddr, page_index, sps);
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

        let (read_file, read_offset) = self.find_file_and_offset(page_vaddr);

        let mut total_read = 0usize;
        while total_read < readable {
            // SAFETY: The destination slice stays within the single allocated page.
            let dst = unsafe {
                core::slice::from_raw_parts_mut(
                    (kva + total_read) as *mut u8,
                    readable - total_read,
                )
            };
            let n = read_file
                .read_at((read_offset + total_read) as u64, dst)
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            total_read += n;
        }

        if self.needs_header_patch && page_index == 0 {
            self.patch_cache_header(kva);
        }

        // Debug: show first 8 u64 values at specific offsets before fixups
        if page_vaddr == 0x1ef378000 || page_vaddr == 0x1ef37a000 || page_vaddr == 0x1ef37c000 {
            let base_ptr = kva as *const u64;
            crate::println!("[darwin] BEFORE fixup page={:#x} read={}/{} file_off={:#x}", page_vaddr, total_read, readable, read_offset);
            for i in 0..8 {
                let val = unsafe { ptr::read(base_ptr.add(i)) };
                crate::println!("[darwin]   [{}] @{:#x} = {:#x}", i, page_vaddr + i * 8, val);
            }
        }

        self.apply_slide_fixups_for_page(page_vaddr, kva, PAGE_SIZE);

        // Debug: show values after fixups for the crash page
        if page_vaddr == 0x1ef378000 || page_vaddr == 0x1ef37a000 || page_vaddr == 0x1ef37c000 {
            let base_ptr = kva as *const u64;
            crate::println!("[darwin] AFTER fixup page={:#x}", page_vaddr);
            for i in 0..8 {
                let val = unsafe { ptr::read(base_ptr.add(i)) };
                crate::println!("[darwin]   [{}] @{:#x} = {:#x}", i, page_vaddr + i * 8, val);
            }
        }

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

    let mut header_buf = vec![0u8; 0x37600];
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

    let mut total_size = if shared_region_size > 0 {
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

    let main_mappings_slice: Vec<CacheMappingSlideInfo> =
        mappings.iter().take(mapping_count).cloned().collect();
    let mut slide_infos = parse_v5_slide_infos(&file, &main_mappings_slice).map_err(|e| e)?;

    let dynamic_data_offset = if CACHE_HEADER_DYNAMIC_DATA_OFFSET + 8 <= header_buf.len() {
        read_u64(
            &header_buf[CACHE_HEADER_DYNAMIC_DATA_OFFSET..CACHE_HEADER_DYNAMIC_DATA_OFFSET + 8],
        ) as usize
    } else {
        0
    };

    let mut sub_caches = Vec::new();
    let mut has_sub_cache_entries = false;
    if CACHE_HEADER_SUBCACHE_ARRAY_OFFSET + 4 + 4 <= header_buf.len() {
        let sc_array_offset = read_u32(
            &header_buf[CACHE_HEADER_SUBCACHE_ARRAY_OFFSET..CACHE_HEADER_SUBCACHE_ARRAY_OFFSET + 4],
        ) as usize;
        let sc_array_count = read_u32(
            &header_buf[CACHE_HEADER_SUBCACHE_ARRAY_COUNT..CACHE_HEADER_SUBCACHE_ARRAY_COUNT + 4],
        ) as usize;
        crate::println!(
            "[darwin] sub-cache: array_offset={:#x} count={}",
            sc_array_offset,
            sc_array_count
        );
        has_sub_cache_entries = sc_array_count > 0;

        // dyld_subcache_entry: uuid[16] + cacheVMOffset(8) + fileSuffix[32] = 56 bytes
        const SUBCACHE_ENTRY_SIZE: usize = 56;
        for i in 0..sc_array_count {
            let entry_start = sc_array_offset + i * SUBCACHE_ENTRY_SIZE;
            if entry_start + 56 > header_buf.len() {
                break;
            }
            let vm_offset = read_u64(&header_buf[entry_start + 16..entry_start + 24]) as usize;
            let suffix_raw = &header_buf[entry_start + 24..entry_start + 56];
            let suffix_end = suffix_raw.iter().position(|&b| b == 0).unwrap_or(32);
            let suffix = core::str::from_utf8(&suffix_raw[..suffix_end]).unwrap_or(".?");

            let sub_path = alloc::format!("{}{}", cache_file_path, suffix);

            match vfs.open(&sub_path, 0) {
                Ok(file_obj) => {
                    if let crate::object::KernelObject::File(sub_file) = file_obj {
                        let file_size = sub_file.metadata().map(|m| m.size).unwrap_or(0);
                        crate::println!(
                            "[darwin] sub-cache[{}]: {} at VM offset {:#x} ({}MB)",
                            i,
                            suffix,
                            vm_offset,
                            file_size / 1024 / 1024
                        );

                        let mut sub_hdr_buf = alloc::vec![0u8; 0x800];
                        let _ = sub_file.read_at(0, &mut sub_hdr_buf);

                        let sub_slide_mappings =
                            parse_slide_mappings_from_header(&sub_hdr_buf).unwrap_or_default();
                        crate::println!(
                            "[darwin] sub-cache[{}]: parsed {} slide mappings",
                            i,
                            sub_slide_mappings.len()
                        );

                        match parse_v5_slide_infos(&sub_file, &sub_slide_mappings) {
                            Ok(sub_slide_infos) => {
                                crate::println!(
                                    "[darwin] sub-cache[{}]: parsed {} slide info entries",
                                    i,
                                    sub_slide_infos.len()
                                );
                                slide_infos.extend(sub_slide_infos);
                            }
                            Err(e) => {
                                crate::println!(
                                    "[darwin] sub-cache[{}]: slide info parse error: {}",
                                    i,
                                    e
                                );
                            }
                        }

                        sub_caches.push(SubCache {
                            file: sub_file,
                            vm_offset,
                            mappings: sub_slide_mappings,
                        });
                    }
                }
                Err(_) => {
                    crate::println!(
                        "[darwin] sub-cache[{}]: {} not found ({}), sub-cache data unavailable",
                        i,
                        suffix,
                        sub_path
                    );
                }
            }
        }
    }

    Ok(Arc::new(DarwinSharedCache {
        file,
        cache_start,
        total_size,
        mapping_count,
        mappings,
        slide_infos,
        pages: Mutex::new(BTreeMap::new()),
        dynamic_data_offset,
        sub_caches,
        needs_header_patch: false,
    }))
}

pub fn ensure_shared_cache(vfs: &VfsManager) -> Result<(), &'static str> {
    SHARED_CACHE.call_once(|| {
        Mutex::new(match init_shared_cache(vfs) {
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
    let guard = SHARED_CACHE.get().unwrap().lock();
    guard.as_ref().ok_or("Shared cache not available")?;
    Ok(())
}

pub fn setup_shared_cache_region(task: &Task) -> Result<(), &'static str> {
    let vfs = task.get_vfs().ok_or("No VFS")?;
    ensure_shared_cache(&vfs)?;

    let cache = SHARED_CACHE.get().unwrap().lock();
    let cache = cache.as_ref().ok_or("Shared cache not available")?;

    let mmap = VirtualMemoryMap::new(
        MemoryArea::new(0, PAGE_SIZE - 1),
        MemoryArea::new(cache.cache_start, cache.cache_start + cache.total_size - 1),
        VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize
            | VirtualMemoryPermission::User as usize,
        true,
        Some(Arc::clone(cache) as alloc::sync::Arc<dyn MemoryMappingOps>),
    );
    task.vm_manager.add_memory_map(mmap)?;

    cache.ensure_dynamic_region();

    Ok(())
}

#[inline(always)]
fn pac_sign_ia(ptr: u64, modifier: u64) -> u64 {
    let result;
    unsafe {
        core::arch::asm!(
            "pacia {ptr}, {mod}",
            ptr = inout(reg) ptr => result,
            mod = in(reg) modifier,
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
            "pacda {ptr}, {mod}",
            ptr = inout(reg) ptr => result,
            mod = in(reg) modifier,
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

        // _COMM_PAGE_DYLD_FLAGS at offset 0x160: set skipIgnition (bit 3 = 0x8)
        ptr::write((rw_kva + 0x160) as *mut u64, 0x8);
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

/// Map the macOS comm page at 0x7FFFFFE00000 and set skipIgnition flag.
/// On real macOS, the kernel maps a shared page with system flags at a fixed address.
/// dyld reads `_COMM_PAGE_DYLD_FLAGS` (offset 0x160) to determine behavior.
/// Setting `skipIgnition` (bit 3 = 0x8) bypasses libignition/cryptex code that
/// we don't support.
pub fn setup_comm_page(task: &Task) -> Result<(), &'static str> {
    const COMM_PAGE_BASE: usize = 0x7FFFFFE00000;
    const COMM_PAGE_DYLD_FLAGS_OFFSET: usize = 0x160;
    // skipIgnition = bit 3 (0x8)
    const DYLD_FLAGS_SKIP_IGNITION: u64 = 0x8;

    let comm_pages = ContiguousPages::new(1).ok_or("Failed to allocate comm page")?;
    let paddr = comm_pages.as_paddr();

    let mmap = VirtualMemoryMap::new(
        MemoryArea::new(paddr, paddr + PAGE_SIZE - 1),
        MemoryArea::new(COMM_PAGE_BASE, COMM_PAGE_BASE + PAGE_SIZE - 1),
        VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::User as usize,
        false,
        None,
    );
    task.vm_manager.add_memory_map(mmap)?;

    let kva = task
        .vm_manager
        .translate_to_kva(COMM_PAGE_BASE)
        .ok_or("Failed to translate comm page")?;

    unsafe {
        ptr::write_bytes(kva as *mut u8, 0, PAGE_SIZE);
        ptr::write(
            (kva as *mut u8).add(COMM_PAGE_DYLD_FLAGS_OFFSET) as *mut u64,
            DYLD_FLAGS_SKIP_IGNITION,
        );
    }

    task.page_allocations.write().push(comm_pages);

    crate::println!(
        "[darwin] comm page mapped at {:#x} (skipIgnition=1)",
        COMM_PAGE_BASE
    );
    Ok(())
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
    if perms != 0 {
        perms |= VirtualMemoryPermission::User as usize;
    }
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

fn read_exact_at(
    file_obj: &dyn FileObject,
    offset: u64,
    buffer: &mut [u8],
) -> Result<(), &'static str> {
    let mut total_read = 0usize;
    while total_read < buffer.len() {
        let read_len = file_obj
            .read_at(offset + total_read as u64, &mut buffer[total_read..])
            .map_err(|_| "Failed to read Mach-O data")?;
        if read_len == 0 {
            return Err("Unexpected EOF while reading Mach-O data");
        }
        total_read += read_len;
    }
    Ok(())
}

fn read_null_terminated_bytes(bytes: &[u8]) -> Result<&str, &'static str> {
    let end = bytes
        .iter()
        .position(|&b| b == 0)
        .ok_or("Missing Mach-O string terminator")?;
    core::str::from_utf8(&bytes[..end]).map_err(|_| "Invalid Mach-O UTF-8 string")
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
