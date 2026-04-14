//! PE/COFF loader for Windows ARM64 binaries.
//!
//! Provides functionality for loading PE executables and DLLs into task memory,
//! handling sections, relocations, imports, exports, and TLS.

pub mod headers;

use alloc::{string::String, vec, vec::Vec};
use core::sync::atomic::Ordering;

use crate::environment::PAGE_SIZE;
use crate::fs::{FileObject, SeekFrom};
use crate::mem::page::ContiguousPages;
use crate::task::Task;
use crate::vm::addr::virt_to_phys;
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};

use self::headers::*;

/// Result of loading a PE binary into a task.
#[derive(Debug, Clone)]
pub struct PeLoadResult {
    pub entry_point: u64,
    pub image_base: u64,
    pub image_size: u64,
    pub is_dll: bool,
    pub subsystem: u16,
}

/// Parsed PE header information (lightweight, without loading).
#[derive(Debug, Clone)]
pub struct PeHeaderInfo {
    pub machine: u16,
    pub is_pe32plus: bool,
    pub entry_point_rva: u32,
    pub image_base: u64,
    pub image_size: u32,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub subsystem: u16,
    pub dll_characteristics: u16,
    pub size_of_headers: u32,
    pub number_of_sections: u16,
    pub characteristics: u16,
    pub is_dll: bool,
}

/// Check if file data looks like a valid PE binary.
pub fn is_pe_binary(data: &[u8]) -> bool {
    if data.len() < 64 {
        return false;
    }
    if read_u16(data, 0) != DOS_MAGIC {
        return false;
    }
    let pe_offset = read_u32(data, DosHeader::LFANEW_OFFSET) as usize;
    if pe_offset + 4 > data.len() {
        return false;
    }
    read_u32(data, pe_offset) == PE_SIGNATURE
}

/// Check if PE is ARM64.
pub fn is_arm64_pe(data: &[u8]) -> bool {
    if !is_pe_binary(data) {
        return false;
    }
    let pe_offset = read_u32(data, DosHeader::LFANEW_OFFSET) as usize;
    let coff_offset = pe_offset + 4;
    if coff_offset + ImageFileHeader::SIZE > data.len() {
        return false;
    }
    let machine = read_u16(data, coff_offset);
    machine == IMAGE_FILE_MACHINE_ARM64
}

/// Parse PE headers without loading into memory.
pub fn parse_pe_headers(data: &[u8]) -> Result<PeHeaderInfo, PeError> {
    if data.len() < 64 {
        return Err(PeError::FileTooSmall);
    }

    if read_u16(data, 0) != DOS_MAGIC {
        return Err(PeError::InvalidDosMagic);
    }

    let pe_offset = read_u32(data, DosHeader::LFANEW_OFFSET) as usize;
    if pe_offset + 4 > data.len() {
        return Err(PeError::InvalidPeSignature);
    }

    if read_u32(data, pe_offset) != PE_SIGNATURE {
        return Err(PeError::InvalidPeSignature);
    }

    let coff_offset = pe_offset + 4;
    if coff_offset + ImageFileHeader::SIZE > data.len() {
        return Err(PeError::FileTooSmall);
    }

    let machine = read_u16(data, coff_offset);
    if machine != IMAGE_FILE_MACHINE_ARM64 {
        return Err(PeError::UnsupportedMachineType(machine));
    }

    let _size_of_optional_header = read_u16(data, coff_offset + 16) as usize;
    let characteristics = read_u16(data, coff_offset + 18);
    let number_of_sections = read_u16(data, coff_offset + 2);
    let is_dll = characteristics & 0x2000 != 0;

    let opt_offset = coff_offset + ImageFileHeader::SIZE;
    if opt_offset + 2 > data.len() {
        return Err(PeError::FileTooSmall);
    }

    let magic = read_u16(data, opt_offset);
    if magic != PE32PLUS_MAGIC {
        return Err(PeError::UnsupportedOptionalMagic(magic));
    }

    if opt_offset + ImageOptionalHeader64::RVA_AND_SIZES_OFFSET + 4 > data.len() {
        return Err(PeError::FileTooSmall);
    }

    let entry_point = read_u32(data, opt_offset + 16);
    let image_base = read_u64(data, opt_offset + 24);
    let section_alignment = read_u32(data, opt_offset + 32);
    let file_alignment = read_u32(data, opt_offset + 36);
    let image_size = read_u32(data, opt_offset + 56);
    let subsystem = read_u16(data, opt_offset + 68);
    let dll_characteristics = read_u16(data, opt_offset + 70);
    let size_of_headers = read_u32(data, opt_offset + 60);

    Ok(PeHeaderInfo {
        machine,
        is_pe32plus: true,
        entry_point_rva: entry_point,
        image_base,
        image_size,
        section_alignment,
        file_alignment,
        subsystem,
        dll_characteristics,
        size_of_headers,
        number_of_sections,
        characteristics,
        is_dll,
    })
}

/// Read data directory entry from PE headers.
pub fn get_data_directory(data: &[u8], index: usize) -> Result<ImageDataDirectory, PeError> {
    let pe_offset = read_u32(data, DosHeader::LFANEW_OFFSET) as usize;
    let coff_offset = pe_offset + 4;
    let opt_offset = coff_offset + ImageFileHeader::SIZE;

    let dd_offset = opt_offset
        + ImageOptionalHeader64::RVA_AND_SIZES_OFFSET
        + 4
        + (index * ImageDataDirectory::SIZE);
    if dd_offset + ImageDataDirectory::SIZE > data.len() {
        return Ok(ImageDataDirectory::default());
    }

    Ok(ImageDataDirectory {
        virtual_address: read_u32(data, dd_offset),
        size: read_u32(data, dd_offset + 4),
    })
}

/// Convert RVA to file offset using section headers.
pub fn rva_to_file_offset(rva: u32, sections: &[ImageSectionHeader]) -> Option<usize> {
    for section in sections {
        let sec_va = section.virtual_address;
        let sec_size = section.virtual_size.max(section.size_of_raw_data);
        if rva >= sec_va && rva < sec_va + sec_size {
            return Some((rva - sec_va + section.pointer_to_raw_data) as usize);
        }
    }
    None
}

/// Read a null-terminated UTF-8 string from file data at a given offset.
pub fn read_string(data: &[u8], offset: usize) -> String {
    if offset >= data.len() {
        return String::new();
    }
    let end = data[offset..]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(data.len() - offset);
    let bytes = &data[offset..offset + end];
    String::from_utf8_lossy(bytes).into_owned()
}

/// Parse section headers from PE data.
pub fn parse_section_headers(
    data: &[u8],
    number_of_sections: u16,
) -> Result<Vec<ImageSectionHeader>, PeError> {
    let pe_offset = read_u32(data, DosHeader::LFANEW_OFFSET) as usize;
    let coff_offset = pe_offset + 4;
    let size_of_optional = read_u16(data, coff_offset + 16) as usize;
    let sections_offset = coff_offset + ImageFileHeader::SIZE + size_of_optional;

    let mut sections = Vec::with_capacity(number_of_sections as usize);
    for i in 0..number_of_sections as usize {
        let offset = sections_offset + i * ImageSectionHeader::SIZE;
        if offset + ImageSectionHeader::SIZE > data.len() {
            return Err(PeError::FileTooSmall);
        }
        let name = {
            let name_start = offset;
            let mut name_arr = [0u8; 8];
            name_arr.copy_from_slice(&data[name_start..name_start + 8]);
            name_arr
        };
        sections.push(ImageSectionHeader {
            name,
            virtual_size: read_u32(data, offset + 8),
            virtual_address: read_u32(data, offset + 12),
            size_of_raw_data: read_u32(data, offset + 16),
            pointer_to_raw_data: read_u32(data, offset + 20),
            pointer_to_relocations: read_u32(data, offset + 24),
            pointer_to_linenumbers: read_u32(data, offset + 28),
            number_of_relocations: read_u16(data, offset + 32),
            number_of_linenumbers: read_u16(data, offset + 34),
            characteristics: read_u32(data, offset + 36),
        });
    }
    Ok(sections)
}

/// Find an exported function by name in a PE binary's export table.
///
/// Returns the RVA of the function, or `None` if not found.
pub fn find_export_by_name(data: &[u8], name: &str) -> Option<u32> {
    let export_dir = match get_data_directory(data, IMAGE_DIRECTORY_ENTRY_EXPORT) {
        Ok(d) => d,
        Err(_) => return None,
    };
    if !export_dir.is_present() {
        return None;
    }

    let info = parse_pe_headers(data).ok()?;
    let sections = parse_section_headers(data, info.number_of_sections).ok()?;

    let dir_offset = rva_to_file_offset(export_dir.virtual_address, &sections)?;
    if dir_offset + ImageExportDirectory::SIZE > data.len() {
        return None;
    }

    let num_names = read_u32(data, dir_offset + 24) as usize;
    let base_ordinal = read_u32(data, dir_offset + 16);
    let names_rva = read_u32(data, dir_offset + 32);
    let ordinals_rva = read_u32(data, dir_offset + 36);
    let functions_rva = read_u32(data, dir_offset + 28);

    let names_offset = match rva_to_file_offset(names_rva, &sections) {
        Some(o) => o,
        None => return None,
    };
    let ordinals_offset = match rva_to_file_offset(ordinals_rva, &sections) {
        Some(o) => o,
        None => return None,
    };
    let functions_offset = match rva_to_file_offset(functions_rva, &sections) {
        Some(o) => o,
        None => return None,
    };

    let target = name.as_bytes();
    for i in 0..num_names {
        let name_rva_offset = names_offset + i * 4;
        if name_rva_offset + 4 > data.len() {
            break;
        }
        let name_rva = read_u32(data, name_rva_offset);
        let name_file_offset = match rva_to_file_offset(name_rva, &sections) {
            Some(off) => off,
            None => continue,
        };

        if name_file_offset >= data.len() {
            continue;
        }

        let name_end = data[name_file_offset..]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(data.len() - name_file_offset);
        if &data[name_file_offset..name_file_offset + name_end] == target {
            let ordinal_offset = ordinals_offset + i * 2;
            if ordinal_offset + 2 > data.len() {
                return None;
            }
            let ordinal = read_u16(data, ordinal_offset) as usize;
            let func_offset = functions_offset + ordinal * 4;
            if func_offset + 4 > data.len() {
                return None;
            }
            let func_rva = read_u32(data, func_offset);

            // Check for forwarder export (RVA points inside export directory)
            let export_start = export_dir.virtual_address as usize;
            let export_end = export_start + export_dir.size as usize;
            if (func_rva as usize) >= export_start && (func_rva as usize) < export_end {
                // Forwarder — not supported
                return None;
            }

            return Some(func_rva);
        }
    }

    None
}

/// Find the ordinal-only export (a function with no name in the name pointer table).
///
/// On Windows ARM64 ntdll.dll, `LdrInitializeThunk` is exported only by ordinal
/// (not by name). This function finds the first function entry that is not referenced
/// by any ordinal in the name pointer table.
///
/// Returns the RVA of the ordinal-only export, or `None` if all exports have names
/// or if no export directory exists.
pub fn find_ordinal_only_export(data: &[u8]) -> Option<u32> {
    let export_dir = match get_data_directory(data, IMAGE_DIRECTORY_ENTRY_EXPORT) {
        Ok(d) => d,
        Err(_) => return None,
    };
    if !export_dir.is_present() {
        return None;
    }

    let info = parse_pe_headers(data).ok()?;
    let sections = parse_section_headers(data, info.number_of_sections).ok()?;

    let dir_offset = rva_to_file_offset(export_dir.virtual_address, &sections)?;
    if dir_offset + ImageExportDirectory::SIZE > data.len() {
        return None;
    }

    let num_functions = read_u32(data, dir_offset + 20) as usize;
    let num_names = read_u32(data, dir_offset + 24) as usize;
    let functions_rva = read_u32(data, dir_offset + 28);
    let ordinals_rva = read_u32(data, dir_offset + 36);

    let functions_offset = rva_to_file_offset(functions_rva, &sections)?;
    let ordinals_offset = rva_to_file_offset(ordinals_rva, &sections)?;

    // Collect ordinals referenced by names into a sorted Vec
    let mut named_ordinals = alloc::vec::Vec::with_capacity(num_names);
    for i in 0..num_names {
        let ordinal_offset = ordinals_offset + i * 2;
        if ordinal_offset + 2 > data.len() {
            break;
        }
        named_ordinals.push(read_u16(data, ordinal_offset) as usize);
    }
    named_ordinals.sort_unstable();

    // Walk function indices, skipping those present in the sorted name ordinals
    let mut name_idx = 0;
    for func_idx in 0..core::cmp::min(num_functions, 65536) {
        while name_idx < named_ordinals.len() && named_ordinals[name_idx] < func_idx {
            name_idx += 1;
        }
        if name_idx < named_ordinals.len() && named_ordinals[name_idx] == func_idx {
            name_idx += 1;
            continue;
        }

        let func_offset = functions_offset + func_idx * 4;
        if func_offset + 4 > data.len() {
            return None;
        }
        let func_rva = read_u32(data, func_offset);

        let export_start = export_dir.virtual_address as usize;
        let export_end = export_start + export_dir.size as usize;
        if (func_rva as usize) >= export_start && (func_rva as usize) < export_end {
            continue;
        }

        return Some(func_rva);
    }

    None
}

/// Map PE headers page(s) into the task's address space.
///
/// On Windows, the PE headers (MZ/PE/section table) are always mapped at the image
/// base as a read-only page. This is required because some exports (notably
/// `LdrInitializeThunk` in ntdll.dll) have RVA 0, pointing to the headers page.
fn map_pe_headers(
    data: &[u8],
    task: &Task,
    base: u64,
    info: &PeHeaderInfo,
    sections: &[ImageSectionHeader],
) -> Result<(), PeError> {
    // Find the lowest section VA to determine headers extent
    let first_section_va = sections
        .iter()
        .map(|s| s.virtual_address as u64)
        .filter(|&va| va > 0)
        .min()
        .unwrap_or(info.image_size as u64);

    if first_section_va == 0 {
        return Ok(()); // No sections, nothing to do
    }

    let headers_size = (info.size_of_headers as u64).min(first_section_va);
    if headers_size == 0 {
        return Ok(());
    }

    let headers_pages = (headers_size as usize + PAGE_SIZE - 1) / PAGE_SIZE;
    let page_alloc = ContiguousPages::new(headers_pages).ok_or(PeError::FileTooSmall)?;
    let ptr = page_alloc.as_ptr() as *mut u8;
    let pm_start = virt_to_phys(ptr as usize);

    // Copy PE header bytes into the allocated pages
    let copy_len = (info.size_of_headers as usize)
        .min(data.len())
        .min(headers_pages * PAGE_SIZE);
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), ptr, copy_len);
    }

    let vmarea = MemoryArea {
        start: base as usize,
        end: (base + (headers_pages as u64) * PAGE_SIZE as u64) as usize - 1,
    };
    let pmarea = MemoryArea {
        start: pm_start,
        end: pm_start + headers_pages * PAGE_SIZE - 1,
    };
    let map = VirtualMemoryMap {
        vmarea,
        pmarea,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::User as usize,
        is_shared: false,
        owner: None,
    };

    if task.vm_manager.add_memory_map(map).is_err() {
        return Err(PeError::FileTooSmall);
    }

    task.page_allocations.write().push(page_alloc);
    Ok(())
}

/// Load a PE binary from raw bytes (not a file object) into a task's memory space.
///
/// This is used for loading bundled DLLs like ntdll.dll that are embedded in the kernel.
pub fn load_pe_from_bytes(
    data: &[u8],
    task: &Task,
    preferred_base: Option<u64>,
) -> Result<PeLoadResult, PeError> {
    let info = parse_pe_headers(data)?;
    let sections = parse_section_headers(data, info.number_of_sections)?;

    let preferred = preferred_base.unwrap_or(info.image_base);
    let size = align_up(info.image_size as u64, PAGE_SIZE as u64);

    let base = if task
        .vm_manager
        .search_memory_map(preferred as usize)
        .is_none()
    {
        preferred
    } else {
        task.vm_manager
            .find_unmapped_area(size as usize, PAGE_SIZE)
            .ok_or(PeError::FileTooSmall)? as u64
    };

    map_pe_headers(data, task, base, &info, &sections)?;

    for section in &sections {
        if section.size_of_raw_data == 0 && section.virtual_size == 0 {
            continue;
        }

        let sec_va = base + section.virtual_address as u64;
        let sec_size = align_up(
            section.virtual_size.max(section.size_of_raw_data) as u64,
            PAGE_SIZE as u64,
        );

        let mut permissions = VirtualMemoryPermission::User as usize;
        if section.is_readable() {
            permissions |= VirtualMemoryPermission::Read as usize;
        }
        if section.is_writable() {
            permissions |= VirtualMemoryPermission::Write as usize;
        }
        if section.is_executable() {
            permissions |= VirtualMemoryPermission::Execute as usize;
        }

        let num_of_pages = (sec_size as usize).div_ceil(PAGE_SIZE);
        let page_alloc = ContiguousPages::new(num_of_pages).ok_or(PeError::FileTooSmall)?;
        let ptr = page_alloc.as_ptr() as *mut u8;
        let pm_start = virt_to_phys(ptr as usize);

        let vmarea = MemoryArea {
            start: sec_va as usize,
            end: (sec_va + sec_size) as usize - 1,
        };
        let pmarea = MemoryArea {
            start: pm_start,
            end: pm_start + sec_size as usize - 1,
        };
        let map = VirtualMemoryMap {
            vmarea,
            pmarea,
            permissions,
            is_shared: false,
            owner: None,
        };

        if task.vm_manager.add_memory_map(map).is_err() {
            return Err(PeError::FileTooSmall);
        }

        task.page_allocations.write().push(page_alloc);
    }

    for section in &sections {
        if section.size_of_raw_data == 0 {
            continue;
        }
        let raw_offset = section.pointer_to_raw_data as usize;
        let copy_len = section.size_of_raw_data.min(section.virtual_size) as usize;

        if raw_offset + copy_len > data.len() {
            return Err(PeError::FileTooSmall);
        }

        let sec_va = base + section.virtual_address as u64;
        let remaining =
            (section.virtual_size as usize).saturating_sub(section.size_of_raw_data as usize);

        for chunk_start in (0..copy_len).step_by(PAGE_SIZE) {
            let chunk_len = (copy_len - chunk_start).min(PAGE_SIZE);
            let vaddr = (sec_va + chunk_start as u64) as usize;

            if let Some(kva) = task.vm_manager.translate_to_kva(vaddr) {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data[raw_offset + chunk_start..].as_ptr(),
                        kva as *mut u8,
                        chunk_len,
                    );
                }
            }
        }

        for i in 0..remaining {
            let vaddr = (sec_va + (copy_len + i) as u64) as usize;
            if let Some(kva) = task.vm_manager.translate_to_kva(vaddr) {
                unsafe {
                    *(kva as *mut u8) = 0;
                }
            }
        }
    }

    if base != info.image_base {
        let reloc_dir = get_data_directory(data, IMAGE_DIRECTORY_ENTRY_BASERELOC)?;
        if reloc_dir.is_present() {
            apply_relocations(data, &sections, base, info.image_base, reloc_dir, task)?;
        }
    }

    let entry_point = base + info.entry_point_rva as u64;

    Ok(PeLoadResult {
        entry_point,
        image_base: base,
        image_size: size,
        is_dll: info.is_dll,
        subsystem: info.subsystem,
    })
}

/// Load a PE binary into a task's memory space.
///
/// Maps PE sections, applies relocations, and returns load info.
/// Does NOT resolve imports or call DllMain — those are separate steps.
pub fn load_pe_into_task(
    file_obj: &dyn FileObject,
    task: &Task,
    preferred_base: Option<u64>,
) -> Result<PeLoadResult, PeError> {
    let file_size = file_obj
        .seek(SeekFrom::End(0))
        .map_err(|_| PeError::FileTooSmall)?;
    file_obj.seek(SeekFrom::Start(0)).ok();

    if file_size < 64 {
        return Err(PeError::FileTooSmall);
    }

    let mut buf = vec![0u8; file_size as usize];
    file_obj.read(&mut buf).map_err(|_| PeError::FileTooSmall)?;

    let info = parse_pe_headers(&buf)?;
    let sections = parse_section_headers(&buf, info.number_of_sections)?;

    let preferred = preferred_base.unwrap_or(info.image_base);
    let size = align_up(info.image_size as u64, PAGE_SIZE as u64);

    let base = if task
        .vm_manager
        .search_memory_map(preferred as usize)
        .is_none()
    {
        preferred
    } else {
        task.vm_manager
            .find_unmapped_area(size as usize, PAGE_SIZE)
            .ok_or(PeError::FileTooSmall)? as u64
    };

    task.text_size.store(0, Ordering::SeqCst);
    task.data_size.store(0, Ordering::SeqCst);
    task.stack_size.store(0, Ordering::SeqCst);
    task.brk.store(usize::MAX, Ordering::SeqCst);

    map_pe_headers(&buf, task, base, &info, &sections)?;

    for section in &sections {
        if section.size_of_raw_data == 0 && section.virtual_size == 0 {
            continue;
        }

        let sec_va = base + section.virtual_address as u64;
        let sec_size = align_up(
            section.virtual_size.max(section.size_of_raw_data) as u64,
            PAGE_SIZE as u64,
        );

        let mut permissions = VirtualMemoryPermission::User as usize;
        if section.is_readable() {
            permissions |= VirtualMemoryPermission::Read as usize;
        }
        if section.is_writable() {
            permissions |= VirtualMemoryPermission::Write as usize;
        }
        if section.is_executable() {
            permissions |= VirtualMemoryPermission::Execute as usize;
        }

        let num_of_pages = (sec_size as usize).div_ceil(PAGE_SIZE);
        let page_alloc = ContiguousPages::new(num_of_pages).ok_or(PeError::FileTooSmall)?;
        let ptr = page_alloc.as_ptr() as *mut u8;
        let pm_start = virt_to_phys(ptr as usize);

        let vmarea = MemoryArea {
            start: sec_va as usize,
            end: (sec_va + sec_size) as usize - 1,
        };
        let pmarea = MemoryArea {
            start: pm_start,
            end: pm_start + sec_size as usize - 1,
        };
        let map = VirtualMemoryMap {
            vmarea,
            pmarea,
            permissions,
            is_shared: false,
            owner: None,
        };

        if let Err(e) = task.vm_manager.add_memory_map(map) {
            let _ = e;
            return Err(PeError::FileTooSmall);
        }

        task.page_allocations.write().push(page_alloc);
    }

    for section in &sections {
        if section.size_of_raw_data == 0 {
            continue;
        }
        let raw_offset = section.pointer_to_raw_data as usize;
        let copy_len = section.size_of_raw_data.min(section.virtual_size) as usize;

        if raw_offset + copy_len > buf.len() {
            return Err(PeError::FileTooSmall);
        }

        let sec_va = base + section.virtual_address as u64;
        let remaining =
            (section.virtual_size as usize).saturating_sub(section.size_of_raw_data as usize);

        for chunk_start in (0..copy_len).step_by(PAGE_SIZE) {
            let chunk_len = (copy_len - chunk_start).min(PAGE_SIZE);
            let vaddr = (sec_va + chunk_start as u64) as usize;

            if let Some(kva) = task.vm_manager.translate_to_kva(vaddr) {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        buf[raw_offset + chunk_start..].as_ptr(),
                        kva as *mut u8,
                        chunk_len,
                    );
                }
            }
        }

        for i in 0..remaining {
            let vaddr = (sec_va + (copy_len + i) as u64) as usize;
            if let Some(kva) = task.vm_manager.translate_to_kva(vaddr) {
                unsafe {
                    *(kva as *mut u8) = 0;
                }
            }
        }
    }

    if base != info.image_base {
        let reloc_dir = get_data_directory(&buf, IMAGE_DIRECTORY_ENTRY_BASERELOC)?;
        if reloc_dir.is_present() {
            apply_relocations(&buf, &sections, base, info.image_base, reloc_dir, task)?;
        }
    }

    let entry_point = base + info.entry_point_rva as u64;

    Ok(PeLoadResult {
        entry_point,
        image_base: base,
        image_size: size,
        is_dll: info.is_dll,
        subsystem: info.subsystem,
    })
}

/// Apply ARM64 PE base relocations to fix up absolute addresses when the
/// image is loaded at a different base than its preferred ImageBase.
///
/// Walks the `.reloc` section block by block. Each block covers a 4 KiB page
/// and contains a list of (type, offset) entries that describe how to patch
/// the loaded image.
fn apply_relocations(
    pe_data: &[u8],
    sections: &[ImageSectionHeader],
    new_base: u64,
    old_base: u64,
    reloc_dir: ImageDataDirectory,
    task: &Task,
) -> Result<(), PeError> {
    let delta = (new_base as i64) - (old_base as i64);
    if delta == 0 {
        return Ok(());
    }

    let reloc_file_start =
        rva_to_file_offset(reloc_dir.virtual_address, sections).ok_or(PeError::RelocationFailed)?;
    let reloc_file_end = reloc_file_start + reloc_dir.size as usize;
    if reloc_file_end > pe_data.len() {
        return Err(PeError::RelocationFailed);
    }

    let mut offset = reloc_file_start;
    while offset + ImageBaseRelocation::SIZE <= reloc_file_end {
        let block_hdr = ImageBaseRelocation {
            virtual_address: read_u32(pe_data, offset),
            size_of_block: read_u32(pe_data, offset + 4),
        };

        if block_hdr.size_of_block == 0 {
            break;
        }
        if block_hdr.size_of_block < ImageBaseRelocation::SIZE as u32 {
            break;
        }

        let page_rva = block_hdr.virtual_address;
        let entry_count = block_hdr.entry_count();
        let entries_start = offset + ImageBaseRelocation::SIZE;

        for i in 0..entry_count {
            let entry_off = entries_start + i * 2;
            if entry_off + 2 > reloc_file_end {
                break;
            }
            let entry = read_u16(pe_data, entry_off);
            let reloc_type = (entry >> 12) & 0xF;
            let reloc_offset = (entry & 0xFFF) as u32;

            if reloc_type == IMAGE_REL_ARM64_ABSOLUTE {
                continue;
            }

            let target_rva = page_rva + reloc_offset;
            let target_va = new_base + target_rva as u64;

            let kva = match task.vm_manager.translate_to_kva(target_va as usize) {
                Some(k) => k,
                None => continue,
            };

            match reloc_type {
                IMAGE_REL_ARM64_ADDR64 => {
                    let old_val = unsafe { core::ptr::read_volatile(kva as *const u64) };
                    let new_val = (old_val as i64 + delta) as u64;
                    unsafe { core::ptr::write_volatile(kva as *mut u64, new_val) };
                }
                IMAGE_REL_ARM64_ADDR32NB => {
                    let new_rva = (target_rva as i64 + delta) as u32;
                    unsafe { core::ptr::write_volatile(kva as *mut u32, new_rva) };
                }
                IMAGE_REL_ARM64_PAGEBASE_REL21 => {
                    // ADRP: immlo=[30:29], immhi=[23:5], sign-extended 21-bit imm << 12
                    let insn = unsafe { core::ptr::read_volatile(kva as *const u32) };
                    let pc_page = target_va & !0xFFFu64;
                    let immlo = ((insn >> 29) & 0x3) as u64;
                    let immhi = ((insn >> 5) & 0x7FFFF) as u64;
                    let imm = (immhi << 2) | immlo;
                    let imm = if imm & (1 << 20) != 0 {
                        (imm | !0x1FFFFF) as i64
                    } else {
                        imm as i64
                    };
                    let original_page = ((old_base + target_rva as u64) & !0xFFFu64)
                        .wrapping_add((imm << 12) as u64);
                    let new_page = (original_page as i64 + delta) as u64;
                    let page_off = (new_page as i64) - (pc_page as i64);
                    let page_off_shifted = page_off >> 12;
                    let new_immlo = (page_off_shifted as u64 & 0x3) as u32;
                    let new_immhi = ((page_off_shifted as u64 >> 2) & 0x7FFFF) as u32;
                    let new_insn = (insn & !(0x3 << 29) & !(0x7FFFF << 5))
                        | (new_immlo << 29)
                        | (new_immhi << 5);
                    unsafe { core::ptr::write_volatile(kva as *mut u32, new_insn) };
                }
                IMAGE_REL_ARM64_PAGEOFFSET_12A => {
                    // ADD imm12: bits [21:10], add delta's low 12 bits
                    let insn = unsafe { core::ptr::read_volatile(kva as *const u32) };
                    let old_imm12 = (insn >> 10) & 0xFFF;
                    let new_imm12 = ((old_imm12 as i64) + (delta & 0xFFF)) as u32 & 0xFFF;
                    let new_insn = (insn & !(0xFFF << 10)) | (new_imm12 << 10);
                    unsafe { core::ptr::write_volatile(kva as *mut u32, new_insn) };
                }
                IMAGE_REL_ARM64_PAGEOFFSET_12L => {
                    // LDR/STR imm12: bits [21:10], scaled by 1<<size ([31:30])
                    let insn = unsafe { core::ptr::read_volatile(kva as *const u32) };
                    let old_imm12 = (insn >> 10) & 0xFFF;
                    let scale = (insn >> 30) & 0x3;
                    let scale_factor = 1u32 << scale;
                    let old_byte_offset = old_imm12 * scale_factor;
                    let new_byte_offset = ((old_byte_offset as i64) + (delta & 0xFFF)) as u32;
                    let new_imm12 = new_byte_offset / scale_factor;
                    let new_insn = (insn & !(0xFFF << 10)) | (new_imm12 << 10);
                    unsafe { core::ptr::write_volatile(kva as *mut u32, new_insn) };
                }
                IMAGE_REL_ARM64_PAGEOFFSET_12A => {
                    // ADD immediate: bits [21:10] = imm12.
                    // The 12-bit offset into the page stays the same (delta low 12 bits are 0
                    // for page-aligned bases). But if old_base low 12 != new_base low 12, we
                    // need to add delta's low 12 bits.
                    let insn = unsafe { core::ptr::read_volatile(kva as *const u32) };
                    let old_imm12 = (insn >> 10) & 0xFFF;
                    // Decode original symbol address from ADRP+ADD pair:
                    // The ADD's imm12 encodes the page offset.
                    let new_imm12 = ((old_imm12 as i64) + (delta & 0xFFF)) as u32 & 0xFFF;
                    let new_insn = (insn & !(0xFFF << 10)) | (new_imm12 << 10);
                    unsafe { core::ptr::write_volatile(kva as *mut u32, new_insn) };
                }
                IMAGE_REL_ARM64_PAGEOFFSET_12L => {
                    // LDR/STR unsigned offset: bits [21:10] = imm12, scaled by element size.
                    let insn = unsafe { core::ptr::read_volatile(kva as *const u32) };
                    let old_imm12 = (insn >> 10) & 0xFFF;
                    // Scale factor depends on instruction size bits [31:30]:
                    //   00 = 8-bit, 01 = 16-bit, 10 = 32-bit, 11 = 64-bit
                    let scale = (insn >> 30) & 0x3;
                    let scale_factor = 1u32 << scale;
                    let old_byte_offset = old_imm12 * scale_factor;
                    let new_byte_offset = ((old_byte_offset as i64) + (delta & 0xFFF)) as u32;
                    let new_imm12 = new_byte_offset / scale_factor;
                    let new_insn = (insn & !(0xFFF << 10)) | (new_imm12 << 10);
                    unsafe { core::ptr::write_volatile(kva as *mut u32, new_insn) };
                }
                IMAGE_REL_ARM64_BRANCH26 => {
                    // B/BL: 26-bit signed immediate in bits [25:0], shifted left 2.
                    // Offset is PC-relative and does NOT change with base relocation
                    // since both source and target move by the same delta.
                    // However, if the target is outside the image, we need to fix it.
                    // For intra-image branches, no fixup needed.
                }
                IMAGE_REL_ARM64_BRANCH19 => {
                    // B.cond: 19-bit signed offset — same as BRANCH26, intra-image.
                }
                IMAGE_REL_ARM64_BRANCH14 => {
                    // TBZ/TBNZ: 14-bit signed offset — same, intra-image.
                }
                IMAGE_REL_ARM64_REL32 => {
                    // 32-bit PC-relative offset: add delta's low 32 bits.
                    // Since both PC and target shift by delta, no change needed
                    // for intra-image references. But for cross-image, fix up.
                    // We apply delta anyway to be safe.
                    let old_val = unsafe { core::ptr::read_volatile(kva as *const u32) };
                    let new_val = ((old_val as i64) + delta) as u32;
                    unsafe { core::ptr::write_volatile(kva as *mut u32, new_val) };
                }
                _ => {
                    // Unknown relocation type — skip silently.
                    // ARM64 SECREL, TOKEN, SECTION types are rare in base relocs.
                }
            }
        }

        offset += block_hdr.size_of_block as usize;
    }

    Ok(())
}
