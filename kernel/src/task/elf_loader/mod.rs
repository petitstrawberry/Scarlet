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
//! - `load_elf_into_task`: Loads an ELF file from a file handle into a task's memory space
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
use crate::environment::PAGE_SIZE;
use crate::fs::{File, SeekFrom};
use crate::mem::page::allocate_pages;
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};
use alloc::{format, vec};
use alloc::string::{String, ToString};
use crate::task::Task;

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

// Segment Flags
pub const PF_X: u32 = 1; // Executable
pub const PF_W: u32 = 2; // Writable
pub const PF_R: u32 = 4; // Readable

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

/// Load an ELF file into a task's memory space
/// 
/// # Arguments
/// 
/// * `file`: A mutable reference to a file handle containing the ELF file
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
pub fn load_elf_into_task(file: &mut File, task: &mut Task) -> Result<u64, ElfLoaderError> {
    // Move to the beginning of the file
    file.seek(SeekFrom::Start(0)).map_err(|e| ElfLoaderError {
        message: format!("Failed to seek to start of file: {:?}", e),
    })?;
    // Read the ELF header
    let mut header_buffer = vec![0u8; 64]; // 64-bit ELF header size
    file.read(&mut header_buffer).map_err(|e| ElfLoaderError {
        message: format!("Failed to read ELF header: {:?}", e),
    })?;
    
    let header = match ElfHeader::parse(&header_buffer) {
        Ok(header) => header,
        Err(e) => return Err(ElfLoaderError {
            message: format!("Failed to parse ELF header: {:?}", e),
        }),
    };
    // Read program headers and load LOAD segments
    for i in 0..header.e_phnum {
        // Seek to the program header position
        let offset = header.e_phoff + (i as u64) * (header.e_phentsize as u64);
        file.seek(SeekFrom::Start(offset)).map_err(|e| ElfLoaderError {
            message: format!("Failed to seek to program header: {:?}", e),
        })?;

        // Read program header
        let mut ph_buffer = vec![0u8; header.e_phentsize as usize];
        file.read(&mut ph_buffer).map_err(|e| ElfLoaderError {
            message: format!("Failed to read program header: {:?}", e),
        })?;
        
        let ph = match ProgramHeader::parse(&ph_buffer, header.ei_data == ELFDATA2LSB) {
            Ok(ph) => ph,
            Err(e) => return Err(ElfLoaderError {
                message: format!("Failed to parse program header: {:?}", e),
            }),
        };

        let aligned_vaddr = if ph.p_align == 0 {
            (ph.p_vaddr + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1)
        } else {
            (ph.p_vaddr + ph.p_align - 1) & !(ph.p_align - 1)
        };

        // For LOAD segments, load them into memory
        if ph.p_type == PT_LOAD {
            // Allocate memory for the segment
           map_elf_segment(task, aligned_vaddr as usize, ph.p_memsz as usize, ph.p_align as usize, ph.p_flags).map_err(|e| ElfLoaderError {
                message: format!("Failed to map ELF segment: {:?}", e),
            })?;
            
            // Prepare segment data (file size)
            let mut segment_data = vec![0u8; ph.p_filesz as usize];
            
            // Seek to segment data position
            file.seek(SeekFrom::Start(ph.p_offset)).map_err(|e| ElfLoaderError {
                message: format!("Failed to seek to segment data: {:?}", e),
            })?;

            // Read segment data
            file.read(&mut segment_data).map_err(|e| ElfLoaderError {
                message: format!("Failed to read segment data: {:?}", e),
            })?;
            
            // Copy data to task's memory space
            let vaddr = aligned_vaddr as usize;
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
    
    // Return the entry point
    Ok(header.e_entry)
}

fn map_elf_segment(task: &mut Task, vaddr: usize, size: usize, align: usize, flags: u32) -> Result<(), &'static str> {
    // Check if the address is aligned
    if align == 0 {
        if vaddr % PAGE_SIZE != 0 {
            return Err("Address is not aligned");
        }
    } else if vaddr % align != 0 {
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

    // Create memory area
    let vmarea = MemoryArea {
        start: vaddr,
        end: vaddr + size - 1,
    };

    // Check if the area is already mapped
    if let Some(_) = task.vm_manager.search_memory_map(vaddr) {
        // If already mapped, do nothing
        return Ok(());
    }

    // Allocate physical memory
    let ptr = allocate_pages((size + PAGE_SIZE - 1) / PAGE_SIZE);
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
    };

    // Add to VM manager
    task.vm_manager.add_memory_map(map);

    Ok(())
}

#[cfg(test)]
mod tests;