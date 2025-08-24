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
use alloc::{format, vec};
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
const PT_INTERP: u32 = 3; // Interpreter path

/// Execution mode determined by ELF analysis
#[derive(Debug, Clone)]
pub enum ExecutionMode {
    /// Static linking - direct execution
    Static,
    /// Dynamic linking - needs interpreter
    Dynamic {
        interpreter_path: String,
    },
}

/// Result of ELF loading analysis
#[derive(Debug, Clone)]
pub struct LoadElfResult {
    /// Execution mode (static or dynamic)
    pub mode: ExecutionMode,
    /// Entry point (either main program or interpreter)
    pub entry_point: u64,
    /// Base address where main program was loaded (for auxiliary vector)
    pub base_address: Option<u64>,
    /// Program headers info (for auxiliary vector)
    pub program_headers: ProgramHeadersInfo,
}

/// Program headers information for auxiliary vector
#[derive(Debug, Clone)]
pub struct ProgramHeadersInfo {
    pub phdr_addr: u64,    // Address of program headers in memory
    pub phdr_size: u64,    // Size of program header entry
    pub phdr_count: u64,   // Number of program headers
}

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
/// Load ELF file into task (backward compatibility wrapper)
/// 
/// This function provides backward compatibility with the existing API.
/// It calls the new analyze_and_load_elf function and returns only the entry point.
/// 
pub fn load_elf_into_task(file_obj: &dyn FileObject, task: &mut Task) -> Result<u64, ElfLoaderError> {
    let result = analyze_and_load_elf(file_obj, task)?;
    Ok(result.entry_point)
}

/// Analyze ELF file and load it with dynamic linking support
/// 
/// This function determines whether the ELF file requires dynamic linking by checking
/// for PT_INTERP segment, then loads either the interpreter (dynamic linker) or the
/// main program directly (static linking).
/// 
/// # Arguments
/// 
/// * `file_obj`: A reference to the file object containing the ELF data
/// * `task`: A mutable reference to the task into which the ELF file will be loaded
/// 
/// # Returns
/// 
/// * `Result<LoadElfResult, ElfLoaderError>`: Information about the loaded ELF including
///   execution mode, entry point, and auxiliary vector data
/// 
pub fn analyze_and_load_elf(file_obj: &dyn FileObject, task: &mut Task) -> Result<LoadElfResult, ElfLoaderError> {
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

    // Step 1: Check for PT_INTERP segment
    let interpreter_path = find_interpreter_path(&header, file_obj)?;
    
    match interpreter_path {
        Some(interp_path) => {
            // Dynamic linking required
            let base_address = load_elf_segments_for_interpreter(&header, file_obj, task)?;
            let interpreter_entry = load_interpreter(&interp_path, task)?;
            
            // Prepare program headers info for auxiliary vector
            let phdr_info = ProgramHeadersInfo {
                phdr_addr: base_address + header.e_phoff,
                phdr_size: header.e_phentsize as u64,
                phdr_count: header.e_phnum as u64,
            };
            
            Ok(LoadElfResult {
                mode: ExecutionMode::Dynamic { interpreter_path: interp_path },
                entry_point: interpreter_entry,
                base_address: Some(base_address),
                program_headers: phdr_info,
            })
        }
        None => {
            // Static linking - use existing implementation
            let entry_point = load_elf_into_task_static(&header, file_obj, task)?;
            
            Ok(LoadElfResult {
                mode: ExecutionMode::Static,
                entry_point,
                base_address: None,
                program_headers: ProgramHeadersInfo {
                    phdr_addr: 0,
                    phdr_size: 0,
                    phdr_count: 0,
                },
            })
        }
    }
}

/// Find PT_INTERP segment and extract interpreter path
fn find_interpreter_path(header: &ElfHeader, file_obj: &dyn FileObject) -> Result<Option<String>, ElfLoaderError> {
    for i in 0..header.e_phnum {
        let offset = header.e_phoff + (i as u64) * (header.e_phentsize as u64);
        file_obj.seek(SeekFrom::Start(offset)).map_err(|e| ElfLoaderError {
            message: format!("Failed to seek to program header: {:?}", e),
        })?;

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

        if ph.p_type == PT_INTERP {
            // Read interpreter path
            file_obj.seek(SeekFrom::Start(ph.p_offset)).map_err(|e| ElfLoaderError {
                message: format!("Failed to seek to interpreter path: {:?}", e),
            })?;
            
            let mut interp_buffer = vec![0u8; ph.p_filesz as usize];
            file_obj.read(&mut interp_buffer).map_err(|e| ElfLoaderError {
                message: format!("Failed to read interpreter path: {:?}", e),
            })?;
            
            // Remove null terminator and convert to string
            if let Some(null_pos) = interp_buffer.iter().position(|&x| x == 0) {
                interp_buffer.truncate(null_pos);
            }
            
            let path = core::str::from_utf8(&interp_buffer)
                .map_err(|_| ElfLoaderError {
                    message: "Invalid UTF-8 in interpreter path".to_string(),
                })?
                .to_string();
                
            return Ok(Some(path));
        }
    }
    
    Ok(None)
}

/// Load ELF segments for dynamic execution (without executing)
fn load_elf_segments_for_interpreter(header: &ElfHeader, file_obj: &dyn FileObject, task: &mut Task) -> Result<u64, ElfLoaderError> {
    // For now, load at fixed base address - in real implementation,
    // this should be position-independent or use ASLR
    let base_address = 0x10000000u64;
    
    // Load PT_LOAD segments
    for i in 0..header.e_phnum {
        let offset = header.e_phoff + (i as u64) * (header.e_phentsize as u64);
        file_obj.seek(SeekFrom::Start(offset)).map_err(|e| ElfLoaderError {
            message: format!("Failed to seek to program header: {:?}", e),
        })?;

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

        if ph.p_type == PT_LOAD {
            // Map segment but don't initialize yet - interpreter will handle initialization
            let segment_addr = base_address + ph.p_vaddr;
            let align = if ph.p_align == 0 { 1 } else { ph.p_align as usize };
            let aligned_size = ((ph.p_memsz as usize) + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            
            map_elf_segment(task, segment_addr as usize, aligned_size, align, ph.p_flags).map_err(|e| ElfLoaderError {
                message: format!("Failed to map ELF segment for interpreter: {:?}", e),
            })?;
            
            // Copy file data to memory
            let mut segment_data = vec![0u8; ph.p_filesz as usize];
            file_obj.seek(SeekFrom::Start(ph.p_offset)).map_err(|e| ElfLoaderError {
                message: format!("Failed to seek to segment data: {:?}", e),
            })?;
            file_obj.read(&mut segment_data).map_err(|e| ElfLoaderError {
                message: format!("Failed to read segment data: {:?}", e),
            })?;
            
            // Write data to task memory
            // Note: This is simplified - real implementation needs proper memory copying
            // task.memory_space.write(segment_addr, &segment_data)?;
        }
    }
    
    Ok(base_address)
}

/// Load interpreter (dynamic linker) into task memory  
fn load_interpreter(interpreter_path: &str, _task: &mut Task) -> Result<u64, ElfLoaderError> {
    // For now, return a placeholder entry point
    // Real implementation should:
    // 1. Open interpreter file from VFS
    // 2. Load interpreter using load_elf_into_task_static
    // 3. Return interpreter's entry point
    
    crate::println!("Would load interpreter: {}", interpreter_path);
    
    // Placeholder - interpreter entry point
    Ok(0x40000000u64)
}

/// Load ELF using the original static linking logic
fn load_elf_into_task_static(header: &ElfHeader, file_obj: &dyn FileObject, task: &mut Task) -> Result<u64, ElfLoaderError> {
    // Read program headers and load LOAD segments (existing logic)
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

        // For LOAD segments, load them into memory
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
                },
                None => {
                    return Err(ElfLoaderError {
                        message: format!("Failed to translate virtual address {:#x}", vaddr),
                    });
                }
            }
        }
    }

    // Return entry point
    Ok(header.e_entry)
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

#[cfg(test)]
mod tests;