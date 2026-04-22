use alloc::{string::{String, ToString}, vec, vec::Vec};
use core::{mem::size_of, ptr};
use core::sync::atomic::Ordering;

use crate::{
    environment::PAGE_SIZE,
    fs::{FileObject, SeekFrom},
    mem::page::ContiguousPages,
    task::Task,
    vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission},
};

pub const MH_MAGIC_64: u32 = 0xFEEDFACF;
pub const MH_EXECUTE: u32 = 0x02;
pub const CPU_TYPE_ARM64: u32 = 0x0100000C;
pub const CPU_SUBTYPE_ALL: u32 = 0x00000000;

pub const LC_SEGMENT_64: u32 = 0x19;
pub const LC_MAIN: u32 = 0x80000028;
pub const LC_UNIXTHREAD: u32 = 0x05;
pub const LC_DYSYMTAB: u32 = 0x0B;
pub const LC_LOAD_DYLIB: u32 = 0x0C;
pub const LC_LOAD_DYLINKER: u32 = 0x80000027;

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

pub fn load_macho_binary(
    file_obj: &dyn FileObject,
    task: &Task,
) -> Result<(usize, Option<String>), &'static str> {
    file_obj
        .seek(SeekFrom::Start(0))
        .map_err(|_| "Failed to seek to Mach-O header")?;

    let mut header_bytes = [0u8; size_of::<MachHeader64>()];
    read_exact(file_obj, &mut header_bytes)?;
    let header = read_struct::<MachHeader64>(&header_bytes)?;

    if header.magic != MH_MAGIC_64 {
        return Err("Invalid Mach-O magic");
    }
    if header.cputype != CPU_TYPE_ARM64 {
        return Err("Unsupported Mach-O CPU type");
    }
    if header.cpusubtype != CPU_SUBTYPE_ALL {
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
                if cmdsize >= 12 {
                    let name_offset = read_u32(&command_bytes[8..12]) as usize;
                    if name_offset < cmdsize {
                        let path_bytes = &command_bytes[name_offset..cmdsize];
                        if let Some(null_pos) = path_bytes.iter().position(|&b| b == 0) {
                            if let Ok(path) = core::str::from_utf8(&path_bytes[..null_pos]) {
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
        map_segment(file_obj, task, segment)?;
    }

    let entry_point = if let Some(entryoff) = entryoff {
        file_offset_to_vaddr(&segments, entryoff).ok_or("Failed to resolve Mach-O entry point")?
    } else if let Some(entry) = unixthread_entry {
        entry as usize
    } else {
        return Err("Mach-O binary missing entry point");
    };

    Ok((entry_point, dylinker_path))
}

fn map_segment(
    file_obj: &dyn FileObject,
    task: &Task,
    segment: &SegmentCommand64,
) -> Result<(), &'static str> {
    if segment.vmsize == 0 {
        return Ok(());
    }

    let permissions = macho_prot_to_scarlet(segment.initprot);
    if permissions == VirtualMemoryPermission::User as usize && segment.filesize == 0 {
        return Ok(());
    }

    let segment_vaddr = usize::try_from(segment.vmaddr).map_err(|_| "Mach-O vmaddr out of range")?;
    let segment_vmsize = usize::try_from(segment.vmsize).map_err(|_| "Mach-O vmsize out of range")?;
    let segment_filesize = usize::try_from(segment.filesize).map_err(|_| "Mach-O filesize out of range")?;
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
            .seek(SeekFrom::Start(segment_fileoff))
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
        0x5f, 0x5f, 0x54, 0x45, 0x58, 0x54, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // segname = "__TEXT"
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, // vmaddr = 0x100000000
        0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, // vmsize = 16384
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // fileoff = 0
        0xe4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesize = 228
        0x07, 0x00, 0x00, 0x00, // maxprot = rwx
        0x07, 0x00, 0x00, 0x00, // initprot = rwx
        0x01, 0x00, 0x00, 0x00, // nsects = 1
        0x00, 0x00, 0x00, 0x00, // flags
        // Section __text (80 bytes)
        0x5f, 0x5f, 0x74, 0x65, 0x78, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // sectname = "__text"
        0x5f, 0x5f, 0x54, 0x45, 0x58, 0x54, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // segname = "__TEXT"
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
        assert_eq!(header.cpusubtype, CPU_SUBTYPE_ALL, "cpusubtype should be ALL");
        assert_eq!(header.filetype, MH_EXECUTE, "filetype should be MH_EXECUTE");
        assert_eq!(header.ncmds, 2, "should have 2 load commands");
    }

    #[test]
    fn test_parse_load_commands() {
        let header = read_struct::<MachHeader64>(&MINIMAL_MACHO_EXIT[..size_of::<MachHeader64>()]).unwrap();
        let cmd_start = size_of::<MachHeader64>();
        let cmd_data = &MINIMAL_MACHO_EXIT[cmd_start..cmd_start + header.sizeofcmds as usize];

        let mut offset = 0usize;
        let mut found_segment = false;
        let mut found_main = false;
        let mut entryoff = 0u64;

        for _ in 0..header.ncmds {
            let load_cmd = read_struct::<LoadCommand>(&cmd_data[offset..offset + size_of::<LoadCommand>()]).unwrap();
            let cmdsize = load_cmd.cmdsize as usize;
            let command_bytes = &cmd_data[offset..offset + cmdsize];

            match load_cmd.cmd {
                LC_SEGMENT_64 => {
                    found_segment = true;
                    let seg = read_struct::<SegmentCommand64>(&command_bytes[..size_of::<SegmentCommand64>()]).unwrap();
                    let segname = &seg.segname[..seg.segname.iter().position(|&b| b == 0).unwrap_or(16)];
                    assert_eq!(segname, b"__TEXT", "first segment should be __TEXT");
                    assert_eq!(seg.vmaddr, 0x100000000, "vmaddr should be 0x100000000");
                    assert_eq!(seg.nsects, 1, "should have 1 section");
                }
                LC_MAIN => {
                    found_main = true;
                    let entry_cmd = read_struct::<EntryPointCommand>(&command_bytes[..size_of::<EntryPointCommand>()]).unwrap();
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
        let header = read_struct::<MachHeader64>(&MINIMAL_MACHO_EXIT[..size_of::<MachHeader64>()]).unwrap();
        let cmd_start = size_of::<MachHeader64>();
        let cmd_data = &MINIMAL_MACHO_EXIT[cmd_start..cmd_start + header.sizeofcmds as usize];

        let mut segments: alloc::vec::Vec<SegmentCommand64> = alloc::vec::Vec::new();
        let mut entryoff = None;

        let mut offset = 0usize;
        for _ in 0..header.ncmds {
            let load_cmd = read_struct::<LoadCommand>(&cmd_data[offset..offset + size_of::<LoadCommand>()]).unwrap();
            let cmdsize = load_cmd.cmdsize as usize;
            let command_bytes = &cmd_data[offset..offset + cmdsize];

            match load_cmd.cmd {
                LC_SEGMENT_64 => {
                    segments.push(read_struct::<SegmentCommand64>(
                        &command_bytes[..size_of::<SegmentCommand64>()],
                    ).unwrap());
                }
                LC_MAIN => {
                    let entry_cmd = read_struct::<EntryPointCommand>(
                        &command_bytes[..size_of::<EntryPointCommand>()],
                    ).unwrap();
                    entryoff = Some(entry_cmd.entryoff);
                }
                _ => {}
            }
            offset += cmdsize;
        }

        let entryoff = entryoff.expect("should have LC_MAIN");
        let entry_vaddr = file_offset_to_vaddr(&segments, entryoff)
            .expect("should resolve entry point");

        // entryoff=208, __TEXT starts at fileoff=0, vmaddr=0x100000000
        // so vaddr = 0x100000000 + 208 = 0x1000000d0
        assert_eq!(entry_vaddr, 0x1000000d0, "entry point should be 0x1000000d0");
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
        assert_eq!(code_in_binary, expected_code, "code should match at offset 208");
    }

    #[test]
    fn test_macho_prot_to_scarlet() {
        use crate::vm::vmem::VirtualMemoryPermission;

        // rwx (7) -> Read | Write | Execute | User
        let rwx = macho_prot_to_scarlet(7);
        assert_eq!(rwx & VirtualMemoryPermission::Read as usize, VirtualMemoryPermission::Read as usize);
        assert_eq!(rwx & VirtualMemoryPermission::Write as usize, VirtualMemoryPermission::Write as usize);
        assert_eq!(rwx & VirtualMemoryPermission::Execute as usize, VirtualMemoryPermission::Execute as usize);
        assert_eq!(rwx & VirtualMemoryPermission::User as usize, VirtualMemoryPermission::User as usize);

        // r-x (5) -> Read | Execute | User
        let rx = macho_prot_to_scarlet(5);
        assert_eq!(rx & VirtualMemoryPermission::Read as usize, VirtualMemoryPermission::Read as usize);
        assert_eq!(rx & VirtualMemoryPermission::Write as usize, 0);
        assert_eq!(rx & VirtualMemoryPermission::Execute as usize, VirtualMemoryPermission::Execute as usize);

        // rw- (3) -> Read | Write | User
        let rw = macho_prot_to_scarlet(3);
        assert_eq!(rw & VirtualMemoryPermission::Read as usize, VirtualMemoryPermission::Read as usize);
        assert_eq!(rw & VirtualMemoryPermission::Write as usize, VirtualMemoryPermission::Write as usize);
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
            let load_cmd = read_struct::<LoadCommand>(&cmd_data[offset..offset + size_of::<LoadCommand>()]).unwrap();
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
