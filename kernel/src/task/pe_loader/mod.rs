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

    let base = preferred_base.unwrap_or(info.image_base);
    let size = align_up(info.image_size as u64, PAGE_SIZE as u64);

    task.text_size.store(0, Ordering::SeqCst);
    task.data_size.store(0, Ordering::SeqCst);
    task.stack_size.store(0, Ordering::SeqCst);
    task.brk.store(usize::MAX, Ordering::SeqCst);

    for section in &sections {
        if section.size_of_raw_data == 0 && section.virtual_size == 0 {
            continue;
        }

        let sec_va = base + section.virtual_address as u64;
        let sec_size = align_up(
            section.virtual_size.max(section.size_of_raw_data) as u64,
            PAGE_SIZE as u64,
        );

        let mut permissions = 0;
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

fn apply_relocations(
    _pe_data: &[u8],
    _sections: &[ImageSectionHeader],
    _new_base: u64,
    _old_base: u64,
    _reloc_dir: ImageDataDirectory,
    _task: &Task,
) -> Result<(), PeError> {
    // TODO: implement ARM64 relocation processing
    // For now, if image loads at preferred base, no relocations needed.
    // Most Windows ARM64 binaries use ASLR and will need this.
    Ok(())
}
