//! ELF Loading Module
//!
//! This module provides functionality for loading ELF (Executable and Linkable Format)
//! executables into a task's memory space. It supports 64-bit ELF files and handles
//! the parsing of ELF headers and program headers, as well as the mapping of loadable
//! segments into memory.
//!
//! # Components
//!
//! - `ElfHeader`: Represents the ELF file header which contains metadata about the file
//! - `ProgramHeader`: Represents a program header which describes a segment in the ELF file
//! - `LoadedSegment`: Represents a segment after it has been loaded into memory
//! - Error types for handling various failure scenarios during ELF parsing and loading
//!
//! # Main Functions
//!
//! - `load_elf_into_task`: Loads an ELF file from a file object into a task's memory space
//! - `map_elf_segment`: Maps an ELF segment into a task's virtual memory
//!
//! # Constants
//!
//! The module defines various constants for ELF parsing, including:
//! - Magic numbers for identifying ELF files
//! - ELF class identifiers (64-bit)
//! - Data encoding formats (little/big endian)
//! - Program header types and segment flags (Read/Write/Execute)
//!
//! # Endian Support
//!
//! The module provides endian-aware data reading functions to correctly parse ELF files
//! regardless of the endianness used in the file.
use core::num;

use crate::environment::PAGE_SIZE;
use crate::fs::{FileObject, SeekFrom};
use crate::mem::page::{allocate_raw_pages, free_raw_pages};
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission, VirtualMemoryRegion};
use alloc::boxed::Box;
use alloc::{format, vec, vec::Vec};
use alloc::string::{String, ToString};
use crate::task::Task;

use super::{ManagedPage, TaskType};

// ELF Magic Number
const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F', ];
// ELF Class
// const ELFCLASS32: u8 = 1; // 32-bit
const ELFCLASS64: u8 = 2; // 64-bit
// ELF Data Endian
const ELFDATA2LSB: u8 = 1; // Little Endian
// const ELFDATA2MSB: u8 = 2; // Big Endian

// Program Header Type
const PT_LOAD: u32 = 1; // Loadable segment
const PT_DYNAMIC: u32 = 2; // Dynamic linking information segment
const PT_INTERP: u32 = 3; // Interpreter pathname

// Segment Flags
pub const PF_X: u32 = 1; // Executable
pub const PF_W: u32 = 2; // Writable
pub const PF_R: u32 = 4; // Readable

// Dynamic Section Tag Types
const DT_NULL: u64 = 0; // End of dynamic section
const DT_NEEDED: u64 = 1; // Name of needed library
const DT_PLTRELSZ: u64 = 2; // Size in bytes of PLT relocs
const DT_PLTGOT: u64 = 3; // Processor defined value
const DT_HASH: u64 = 4; // Address of symbol hash table
const DT_STRTAB: u64 = 5; // Address of string table
const DT_SYMTAB: u64 = 6; // Address of symbol table
const DT_RELA: u64 = 7; // Address of Rela relocs
const DT_RELASZ: u64 = 8; // Total size of Rela relocs
const DT_RELAENT: u64 = 9; // Size of one Rela reloc
const DT_STRSZ: u64 = 10; // Size of string table
const DT_SYMENT: u64 = 11; // Size of one symbol table entry
const DT_INIT: u64 = 12; // Address of init function
const DT_FINI: u64 = 13; // Address of termination function
const DT_SONAME: u64 = 14; // Name of shared object
const DT_RPATH: u64 = 15; // Library search path
const DT_SYMBOLIC: u64 = 16; // Start symbol search here
const DT_REL: u64 = 17; // Address of Rel relocs
const DT_RELSZ: u64 = 18; // Total size of Rel relocs
const DT_RELENT: u64 = 19; // Size of one Rel reloc
const DT_PLTREL: u64 = 20; // Type of reloc in PLT
const DT_DEBUG: u64 = 21; // For debugging; unspecified
const DT_TEXTREL: u64 = 22; // Reloc might modify .text
const DT_JMPREL: u64 = 23; // Address of PLT relocs

// ELF Identifier Indices
const EI_MAG0: usize = 0;
const EI_MAG1: usize = 1;
const EI_MAG2: usize = 2;
const EI_MAG3: usize = 3;
const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;
// const EI_VERSION: usize = 6;

// Endian-aware data reading functions
fn read_u16(buffer: &[u8], offset: usize, is_little_endian: bool) -> u16 {
    let bytes = buffer[offset..offset+2].try_into().unwrap();
    if is_little_endian {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    }
}

fn read_u32(buffer: &[u8], offset: usize, is_little_endian: bool) -> u32 {
    let bytes = buffer[offset..offset+4].try_into().unwrap();
    if is_little_endian {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}

fn read_u64(buffer: &[u8], offset: usize, is_little_endian: bool) -> u64 {
    let bytes = buffer[offset..offset+8].try_into().unwrap();
    if is_little_endian {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    }
}

#[derive(Debug)]
pub struct ElfHeader {
    pub ei_class: u8,      // 32-bit or 64-bit (EI_CLASS)
    pub ei_data: u8,       // Endianness (EI_DATA)
    pub e_type: u16,       // File type
    pub e_machine: u16,    // Machine type
    pub e_version: u32,    // ELF version
    pub e_entry: u64,      // Entry point address
    pub e_phoff: u64,      // Program header table file offset
    pub e_shoff: u64,      // Section header table file offset
    pub e_flags: u32,      // Processor-specific flags
    pub e_ehsize: u16,     // ELF header size
    pub e_phentsize: u16,  // Program header table entry size
    pub e_phnum: u16,      // Number of program header entries
    pub e_shentsize: u16,  // Section header table entry size
    pub e_shnum: u16,      // Number of section header entries
    pub e_shstrndx: u16,   // Section header string table index
}

#[derive(Debug)]
pub struct ProgramHeader {
    pub p_type: u32,       // Segment type
    pub p_flags: u32,      // Segment flags
    pub p_offset: u64,     // Segment offset in file
    pub p_vaddr: u64,      // Segment virtual address for loading
    pub p_paddr: u64,      // Segment physical address (usually unused)
    pub p_filesz: u64,     // Segment size in file
    pub p_memsz: u64,      // Segment size in memory
    pub p_align: u64,      // Segment alignment
}

#[derive(Debug)]
pub enum ElfHeaderParseErrorKind {
    InvalidMagicNumber,
    UnsupportedClass,
    InvalidData,
    Other(String),
}

#[derive(Debug)]
pub struct ElfHeaderParseError {
    pub kind: ElfHeaderParseErrorKind,
    pub message: String,
}

#[derive(Debug)]
pub enum ProgramHeaderParseErrorKind {
    InvalidSize,
    Other(String),
}

#[derive(Debug)]
pub struct ProgramHeaderParseError {
    pub kind: ProgramHeaderParseErrorKind,
    pub message: String,
}

#[derive(Debug)]
pub struct ElfLoaderError {
    pub message: String,
}

impl ElfHeader {
    pub fn parse(buffer: &[u8]) -> Result<Self, ElfHeaderParseError> {
        if buffer.len() < 64 {
            return Err(ElfHeaderParseError {
                kind: ElfHeaderParseErrorKind::InvalidData,
                message: "ELF header too small".to_string(),
            });
        }

        if buffer[EI_MAG0] != ELFMAG[0] || buffer[EI_MAG1] != ELFMAG[1] || 
           buffer[EI_MAG2] != ELFMAG[2] || buffer[EI_MAG3] != ELFMAG[3] {
            return Err(ElfHeaderParseError {
                kind: ElfHeaderParseErrorKind::InvalidMagicNumber,
                message: "Invalid ELF magic number".to_string(),
            });
        }

        let ei_class = buffer[EI_CLASS];
        if ei_class != ELFCLASS64 {
            return Err(ElfHeaderParseError {
                kind: ElfHeaderParseErrorKind::UnsupportedClass,
                message: "Only 64-bit ELF is supported".to_string(),
            });
        }

        // Read each field considering endianness
        let ei_data = buffer[EI_DATA];
        let is_little_endian = ei_data == ELFDATA2LSB;
        let e_type = read_u16(buffer, 16, is_little_endian);
        let e_machine = read_u16(buffer, 18, is_little_endian);
        let e_version = read_u32(buffer, 20, is_little_endian);
        let e_entry = read_u64(buffer, 24, is_little_endian);
        let e_phoff = read_u64(buffer, 32, is_little_endian);
        let e_shoff = read_u64(buffer, 40, is_little_endian);
        let e_flags = read_u32(buffer, 48, is_little_endian);
        let e_ehsize = read_u16(buffer, 52, is_little_endian);
        let e_phentsize = read_u16(buffer, 54, is_little_endian);
        let e_phnum = read_u16(buffer, 56, is_little_endian);
        let e_shentsize = read_u16(buffer, 58, is_little_endian);
        let e_shnum = read_u16(buffer, 60, is_little_endian);
        let e_shstrndx = read_u16(buffer, 62, is_little_endian);

        Ok(Self {
            ei_class,
            ei_data,
            e_type,
            e_machine,
            e_version,
            e_entry,
            e_phoff,
            e_shoff,
            e_flags,
            e_ehsize,
            e_phentsize,
            e_phnum,
            e_shentsize,
            e_shnum,
            e_shstrndx,
        })
    }
}

impl DynamicEntry {
    pub fn parse(buffer: &[u8], is_little_endian: bool) -> Result<Self, String> {
        if buffer.len() < 16 {
            return Err("Dynamic entry too small".to_string());
        }

        let d_tag = read_u64(buffer, 0, is_little_endian);
        let d_val = read_u64(buffer, 8, is_little_endian);

        Ok(Self { d_tag, d_val })
    }
}

impl ProgramHeader {
    pub fn parse(buffer: &[u8], is_little_endian: bool) -> Result<Self, ProgramHeaderParseError> {
        if buffer.len() < 56 {
            return Err(ProgramHeaderParseError {
                kind: ProgramHeaderParseErrorKind::InvalidSize,
                message: "Program header too small".to_string(),
            });
        }

        // Read each field considering endianness
        let p_type = read_u32(buffer, 0, is_little_endian);
        let p_flags = read_u32(buffer, 4, is_little_endian);
        let p_offset = read_u64(buffer, 8, is_little_endian);
        let p_vaddr = read_u64(buffer, 16, is_little_endian);
        let p_paddr = read_u64(buffer, 24, is_little_endian);
        let p_filesz = read_u64(buffer, 32, is_little_endian);
        let p_memsz = read_u64(buffer, 40, is_little_endian);
        let p_align = read_u64(buffer, 48, is_little_endian);

        Ok(Self {
            p_type,
            p_flags,
            p_offset,
            p_vaddr,
            p_paddr,
            p_filesz,
            p_memsz,
            p_align,
        })
    }
}

#[derive(Debug)]
pub struct LoadedSegment {
    pub vaddr: u64,        // Virtual address
    pub size: u64,         // Size
    pub flags: u32,        // Flags (R/W/X)
}

/// Dynamic section entry
#[derive(Debug, Clone)]
pub struct DynamicEntry {
    pub d_tag: u64,        // Dynamic entry type
    pub d_val: u64,        // Value (address, size, etc.)
}

/// Dynamic linking information for a loaded ELF
#[derive(Debug, Default)]
pub struct DynamicInfo {
    pub needed_libraries: Vec<String>,  // Libraries needed (DT_NEEDED)
    pub symbol_table: Option<u64>,      // Address of symbol table (DT_SYMTAB)
    pub string_table: Option<u64>,      // Address of string table (DT_STRTAB)
    pub string_table_size: Option<u64>, // Size of string table (DT_STRSZ)
    pub rela_table: Option<u64>,        // Address of RELA relocations (DT_RELA)
    pub rela_table_size: Option<u64>,   // Size of RELA relocations (DT_RELASZ)
    pub rel_table: Option<u64>,         // Address of REL relocations (DT_REL)
    pub rel_table_size: Option<u64>,    // Size of REL relocations (DT_RELSZ)
    pub init_function: Option<u64>,     // Address of init function (DT_INIT)
    pub fini_function: Option<u64>,     // Address of fini function (DT_FINI)
    pub interpreter_path: Option<String>, // Dynamic linker path (PT_INTERP)
}

/// ELF symbol table entry (64-bit)
#[derive(Debug, Clone)]
pub struct ElfSymbol {
    pub st_name: u32,     // Symbol name (string table index)
    pub st_info: u8,      // Symbol type and binding
    pub st_other: u8,     // Symbol visibility
    pub st_shndx: u16,    // Section index
    pub st_value: u64,    // Symbol value
    pub st_size: u64,     // Symbol size
}

/// Relocation entry with addend (RELA)
#[derive(Debug, Clone)]
pub struct ElfRelocationA {
    pub r_offset: u64,    // Address
    pub r_info: u64,      // Relocation type and symbol index
    pub r_addend: i64,    // Addend
}

/// Relocation entry without addend (REL)
#[derive(Debug, Clone)]
pub struct ElfRelocation {
    pub r_offset: u64,    // Address
    pub r_info: u64,      // Relocation type and symbol index
}

/// Load an ELF file into a task's memory space
/// 
/// # Arguments
/// 
/// * `file`: A mutable reference to a file object containing the ELF file
/// * `task`: A mutable reference to the task into which the ELF file will be loaded
/// 
/// # Returns
/// 
/// * `Result<u64, ElfLoaderError>`: The entry point address of the loaded ELF file on success,
///  or an `ElfLoaderError` on failure
/// 
/// # Errors
/// 
/// * `ElfLoaderError`: If any error occurs during the loading process, such as file read errors,
///  parsing errors, or memory allocation errors
/// 
pub fn load_elf_into_task(file_obj: &dyn FileObject, task: &mut Task) -> Result<u64, ElfLoaderError> {
    let dynamic_info = load_elf_into_task_with_dynamic_info(file_obj, task)?;
    Ok(dynamic_info.0)
}

/// Load an ELF file into a task's memory space and return dynamic linking information
/// 
/// # Arguments
/// 
/// * `file`: A mutable reference to a file object containing the ELF file
/// * `task`: A mutable reference to the task into which the ELF file will be loaded
/// 
/// # Returns
/// 
/// * `Result<(u64, DynamicInfo), ElfLoaderError>`: A tuple containing the entry point address 
///   and dynamic linking information on success, or an `ElfLoaderError` on failure
/// 
pub fn load_elf_into_task_with_dynamic_info(file_obj: &dyn FileObject, task: &mut Task) -> Result<(u64, DynamicInfo), ElfLoaderError> {
    // Move to the beginning of the file
    file_obj.seek(SeekFrom::Start(0)).map_err(|e| ElfLoaderError {
        message: format!("Failed to seek to start of file: {:?}", e),
    })?;
    // Read the ELF header
    let mut header_buffer = vec![0u8; 64]; // 64-bit ELF header size
    file_obj.read(&mut header_buffer).map_err(|e| ElfLoaderError {
        message: format!("Failed to read ELF header: {:?}", e),
    })?;
    
    let header = match ElfHeader::parse(&header_buffer) {
        Ok(header) => header,
        Err(e) => return Err(ElfLoaderError {
            message: format!("Failed to parse ELF header: {:?}", e),
        }),
    };
    
    let mut dynamic_info = DynamicInfo::default();
    
    // First pass: collect information about all segments
    let mut program_headers = Vec::new();
    for i in 0..header.e_phnum {
        // Seek to the program header position
        let offset = header.e_phoff + (i as u64) * (header.e_phentsize as u64);
        file_obj.seek(SeekFrom::Start(offset)).map_err(|e| ElfLoaderError {
            message: format!("Failed to seek to program header: {:?}", e),
        })?;

        // Read program header
        let mut ph_buffer = vec![0u8; header.e_phentsize as usize];
        file_obj.read(&mut ph_buffer).map_err(|e| ElfLoaderError {
            message: format!("Failed to read program header: {:?}", e),
        })?;
        
        let ph = match ProgramHeader::parse(&ph_buffer, header.ei_data == ELFDATA2LSB) {
            Ok(ph) => ph,
            Err(e) => return Err(ElfLoaderError {
                message: format!("Failed to parse program header: {:?}", e),
            }),
        };
        
        program_headers.push(ph);
    }
    
    // Second pass: process PT_INTERP and PT_DYNAMIC segments to collect metadata
    for ph in &program_headers {
        match ph.p_type {
            PT_INTERP => {
                // Read interpreter path
                if ph.p_filesz > 0 && ph.p_filesz < 1024 { // Reasonable size limit
                    file_obj.seek(SeekFrom::Start(ph.p_offset)).map_err(|e| ElfLoaderError {
                        message: format!("Failed to seek to interpreter segment: {:?}", e),
                    })?;
                    
                    let mut interp_data = vec![0u8; ph.p_filesz as usize];
                    file_obj.read(&mut interp_data).map_err(|e| ElfLoaderError {
                        message: format!("Failed to read interpreter segment: {:?}", e),
                    })?;
                    
                    // Convert to string, removing null terminator
                    if let Some(null_pos) = interp_data.iter().position(|&b| b == 0) {
                        interp_data.truncate(null_pos);
                    }
                    if let Ok(interp_path) = String::from_utf8(interp_data) {
                        dynamic_info.interpreter_path = Some(interp_path);
                    }
                }
            },
            PT_DYNAMIC => {
                // We'll process dynamic entries after loading segments into memory
                // since we need to access them through virtual addresses
            },
            _ => {
                // Other segment types don't affect dynamic linking metadata at this stage
            }
        }
    }
    
    // Third pass: load LOAD segments into memory
    for ph in &program_headers {
        if ph.p_type == PT_LOAD {
            // Calculate proper alignment-aware mapping
            let align = ph.p_align as usize;
            // Handle zero or invalid alignment (ELF spec: 0 or 1 means no alignment constraint)
            if align == 0 {
                return Err(ElfLoaderError {
                    message: format!("Invalid alignment value: segment has zero alignment requirement"),
                });
            }
            let page_offset = (ph.p_vaddr as usize) % align;
            let mapping_start = (ph.p_vaddr as usize) - page_offset;
            let mapping_size = (ph.p_memsz as usize) + page_offset;
            
            // Align to page boundaries for actual allocation
            let aligned_size = (mapping_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            
            // Allocate memory for the segment with proper alignment handling
            map_elf_segment(task, mapping_start, aligned_size, align, ph.p_flags).map_err(|e| ElfLoaderError {
                message: format!("Failed to map ELF segment: {:?}", e),
            })?;


            // Inference segment type
            let segment_type = if ph.p_flags & PF_X != 0 {
                VirtualMemoryRegion::Text
            } else if ph.p_flags & PF_W != 0 || ph.p_flags & PF_R != 0 {
                VirtualMemoryRegion::Data
            } else {
                VirtualMemoryRegion::Unknown
            };

            match segment_type {
                VirtualMemoryRegion::Text => {
                    task.text_size += aligned_size as usize;
                },
                VirtualMemoryRegion::Data => {
                    task.data_size += aligned_size as usize;
                },
                _ => {
                    return Err(ElfLoaderError {
                        message: format!("Unknown segment type: {:#x}", ph.p_flags),
                    });
                }
            }
            
            // Prepare segment data (file size)
            let mut segment_data = vec![0u8; ph.p_filesz as usize];
            
            // Seek to segment data position
            file_obj.seek(SeekFrom::Start(ph.p_offset)).map_err(|e| ElfLoaderError {
                message: format!("Failed to seek to segment data: {:?}", e),
            })?;

            // Read segment data
            file_obj.read(&mut segment_data).map_err(|e| ElfLoaderError {
                message: format!("Failed to read segment data: {:?}", e),
            })?;
            
            // Copy data to task's memory space
            let vaddr = ph.p_vaddr as usize;
            match task.vm_manager.translate_vaddr(vaddr) {
                Some(paddr) => {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            segment_data.as_ptr(),
                            paddr as *mut u8,
                            ph.p_filesz as usize
                        );
                    }
                    
                    // If memory size is larger than file size (e.g., BSS segment), fill the rest with zeros
                    if ph.p_memsz > ph.p_filesz {
                        let zero_start = paddr + ph.p_filesz as usize;
                        let zero_size = ph.p_memsz as usize - ph.p_filesz as usize;
                        unsafe {
                            core::ptr::write_bytes(zero_start as *mut u8, 0, zero_size);
                        }
                    }
                },
                None => return Err(ElfLoaderError {
                    message: format!("Failed to translate virtual address: {:#x} for segment at offset {:#x}", vaddr, ph.p_offset),
                }),
            }
        }
    }
    
    // Fourth pass: process PT_DYNAMIC segments now that memory is loaded
    for ph in &program_headers {
        if ph.p_type == PT_DYNAMIC {
            let dynamic_entries = parse_dynamic_section(task, ph.p_vaddr, ph.p_memsz, header.ei_data == ELFDATA2LSB).map_err(|e| ElfLoaderError {
                message: format!("Failed to parse dynamic section: {}", e),
            })?;
            
            process_dynamic_entries(&mut dynamic_info, &dynamic_entries, task).map_err(|e| ElfLoaderError {
                message: format!("Failed to process dynamic entries: {}", e),
            })?;
        }
    }
    
    // Return the entry point and dynamic information
    Ok((header.e_entry, dynamic_info))
}

fn map_elf_segment(task: &mut Task, vaddr: usize, size: usize, align: usize, flags: u32) -> Result<(), &'static str> {
    // Ensure alignment is greater than zero
    if align == 0 {
        return Err("Alignment must be greater than zero");
    }
    // Check if the size is valid
    if size == 0 || size % align != 0 {
        return Err("Invalid size");
    }
    // Check if the address is aligned
    if vaddr % align != 0 {
        return Err("Address is not aligned");
    }

    // Convert flags to VirtualMemoryPermission
    let mut permissions = 0;
    if flags & PF_R != 0 {
        permissions |= VirtualMemoryPermission::Read as usize;
    }
    if flags & PF_W != 0 {
        permissions |= VirtualMemoryPermission::Write as usize;
    }
    if flags & PF_X != 0 {
        permissions |= VirtualMemoryPermission::Execute as usize;
    }
    if task.task_type == TaskType::User {
        permissions |= VirtualMemoryPermission::User as usize;
    }

    // Create memory area
    let vmarea = MemoryArea {
        start: vaddr,
        end: vaddr + size - 1,
    };

    // Check if the area is overlapping with existing mappings
    if task.vm_manager.search_memory_map(vaddr).is_some() {
        return Err("Memory area overlaps with existing mapping");
    }

    // Allocate physical memory
    let num_of_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let pages = allocate_raw_pages(num_of_pages);
    let ptr = pages as *mut u8;
    if ptr.is_null() {
        return Err("Failed to allocate memory");
    }
    let pmarea = MemoryArea {
        start: ptr as usize,
        end: (ptr as usize) + size - 1,
    };

    // Create memory mapping
    let map = VirtualMemoryMap {
        vmarea,
        pmarea,
        permissions,
        is_shared: false, // User program memory should not be shared
        owner: None,
    };

    // Add to VM manager
     if let Err(e) = task.vm_manager.add_memory_map(map) {
        free_raw_pages(pages, num_of_pages);
        return Err(e);
    }

    // Manage segment page in the task
    for i in 0..num_of_pages {
        task.add_managed_page(ManagedPage {
            vaddr: vaddr + i * PAGE_SIZE,
            page: unsafe { Box::from_raw(pages.wrapping_add(i)) },
        });
    }

    Ok(())
}

/// Parse dynamic section entries from memory
fn parse_dynamic_section(task: &Task, vaddr: u64, size: u64, is_little_endian: bool) -> Result<Vec<DynamicEntry>, String> {
    let mut entries = Vec::new();
    let entry_size = 16; // Size of dynamic entry (8 + 8 bytes)
    let num_entries = size / entry_size;
    
    for i in 0..num_entries {
        let entry_vaddr = vaddr + i * entry_size;
        let paddr = task.vm_manager.translate_vaddr(entry_vaddr as usize)
            .ok_or_else(|| format!("Failed to translate dynamic entry address: {:#x}", entry_vaddr))?;
        
        let entry_data: [u8; 16] = unsafe {
            core::ptr::read(paddr as *const [u8; 16])
        };
        
        let entry = DynamicEntry::parse(&entry_data, is_little_endian)?;
        
        if entry.d_tag == DT_NULL {
            break; // End of dynamic section
        }
        
        entries.push(entry);
    }
    
    Ok(entries)
}

/// Read a null-terminated string from virtual memory
fn read_string_from_vaddr(task: &Task, vaddr: u64, max_length: usize) -> Result<String, String> {
    let mut result = Vec::new();
    let mut current_addr = vaddr;
    
    for _ in 0..max_length {
        let paddr = task.vm_manager.translate_vaddr(current_addr as usize)
            .ok_or_else(|| format!("Failed to translate string address: {:#x}", current_addr))?;
        
        let byte = unsafe { core::ptr::read(paddr as *const u8) };
        
        if byte == 0 {
            break; // Null terminator found
        }
        
        result.push(byte);
        current_addr += 1;
    }
    
    String::from_utf8(result)
        .map_err(|e| format!("Invalid UTF-8 string: {}", e))
}

/// Process dynamic entries to populate DynamicInfo
fn process_dynamic_entries(
    dynamic_info: &mut DynamicInfo,
    entries: &[DynamicEntry],
    task: &Task,
) -> Result<(), String> {
    // First pass: collect addresses for string table and other key structures
    let mut string_table_addr: Option<u64> = None;
    let mut string_table_size: Option<u64> = None;
    let mut needed_offsets: Vec<u64> = Vec::new();
    
    for entry in entries {
        match entry.d_tag {
            DT_STRTAB => {
                string_table_addr = Some(entry.d_val);
            },
            DT_STRSZ => {
                string_table_size = Some(entry.d_val);
            },
            DT_NEEDED => {
                needed_offsets.push(entry.d_val);
            },
            DT_SYMTAB => {
                dynamic_info.symbol_table = Some(entry.d_val);
            },
            DT_RELA => {
                dynamic_info.rela_table = Some(entry.d_val);
            },
            DT_RELASZ => {
                dynamic_info.rela_table_size = Some(entry.d_val);
            },
            DT_REL => {
                dynamic_info.rel_table = Some(entry.d_val);
            },
            DT_RELSZ => {
                dynamic_info.rel_table_size = Some(entry.d_val);
            },
            DT_INIT => {
                dynamic_info.init_function = Some(entry.d_val);
            },
            DT_FINI => {
                dynamic_info.fini_function = Some(entry.d_val);
            },
            _ => {
                // Ignore unknown dynamic entry types for now
            }
        }
    }
    
    // Store the string table information
    dynamic_info.string_table = string_table_addr;
    dynamic_info.string_table_size = string_table_size;
    
    // Second pass: resolve library names using string table
    if let (Some(strtab_addr), Some(strtab_size)) = (string_table_addr, string_table_size) {
        for offset in needed_offsets {
            if offset < strtab_size {
                let lib_name_addr = strtab_addr + offset;
                match read_string_from_vaddr(task, lib_name_addr, 256) { // 256 byte limit for library names
                    Ok(lib_name) => {
                        if !lib_name.is_empty() {
                            dynamic_info.needed_libraries.push(lib_name);
                        }
                    },
                    Err(_) => {
                        // If we can't read the string, fall back to a placeholder
                        dynamic_info.needed_libraries.push(format!("library_at_offset_{}", offset));
                    }
                }
            } else {
                // Invalid offset, use placeholder
                dynamic_info.needed_libraries.push(format!("invalid_library_offset_{}", offset));
            }
        }
    } else {
        // No string table available, use placeholders
        for (i, _) in needed_offsets.iter().enumerate() {
            dynamic_info.needed_libraries.push(format!("library_{}", i));
        }
    }
    
    Ok(())
}

#[cfg(test)]
mod tests;