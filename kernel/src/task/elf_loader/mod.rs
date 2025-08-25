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

// ELF File Type
pub const ET_EXEC: u16 = 2; // Executable file
pub const ET_DYN: u16 = 3;  // Shared object file / Position Independent Executable

// Program Header Type
const PT_LOAD: u32 = 1; // Loadable segment
const PT_INTERP: u32 = 3; // Interpreter path

/// Target type for ELF loading (determines base address strategy)
#[derive(Debug, Clone, Copy)]
pub enum LoadTarget {
    MainProgram,  // Main executable being loaded
    Interpreter,  // Dynamic linker/interpreter 
    SharedLib,    // Shared library (future use)
}

/// Binary loading strategy (format-agnostic)
/// 
/// This structure allows ABI modules to customize how binaries are loaded
/// without being tied to specific binary formats like ELF.
pub struct LoadStrategy {
    pub choose_base_address: fn(target: LoadTarget, needs_relocation: bool) -> u64,
    pub resolve_interpreter: fn(requested: Option<&str>) -> Option<String>,
}

impl Default for LoadStrategy {
    fn default() -> Self {
        Self {
            choose_base_address: |target, needs_relocation| {
                match (target, needs_relocation) {
                    (LoadTarget::MainProgram, false) => 0,        // Absolute addresses
                    (LoadTarget::MainProgram, true) => 0x10000,   // PIE executable
                    (LoadTarget::Interpreter, _) => 0x40000000,   // Dynamic linker
                    (LoadTarget::SharedLib, _) => 0x50000000,     // Shared libraries
                }
            },
            resolve_interpreter: |requested| requested.map(|s| s.to_string()),
        }
    }
}

/// Choose base address for ELF loading based on type and target
fn choose_base_address(elf_type: u16, target: LoadTarget) -> u64 {
    match (elf_type, target) {
        // ET_EXEC: Use absolute addresses from ELF file (no base offset)
        (ET_EXEC, _) => 0,
        
        // ET_DYN: Choose appropriate base address for relocation
        (ET_DYN, LoadTarget::MainProgram) => {
            // PIE main program: low memory area
            0x10000  // 64KB - avoid null pointer region
        },
        (ET_DYN, LoadTarget::Interpreter) => {
            // Dynamic linker: high memory area to avoid conflicts
            0x40000000  // 1GB - separate from main program space
        },
        (ET_DYN, LoadTarget::SharedLib) => {
            // Shared libraries: medium memory area
            0x50000000  // 1.25GB - between interpreter and heap
        },
        
        // Unknown type: fallback to main program strategy
        _ => 0x10000,
    }
}

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

// Auxiliary Vector (auxv) types for dynamic linking
/// Auxiliary Vector entry type constants
pub const AT_NULL: u64 = 0;     // End of vector
pub const AT_IGNORE: u64 = 1;   // Entry should be ignored
pub const AT_EXECFD: u64 = 2;   // File descriptor of program
pub const AT_PHDR: u64 = 3;     // Program headers for program
pub const AT_PHENT: u64 = 4;    // Size of program header entry
pub const AT_PHNUM: u64 = 5;    // Number of program headers
pub const AT_PAGESZ: u64 = 6;   // System page size
pub const AT_BASE: u64 = 7;     // Base address of interpreter
pub const AT_FLAGS: u64 = 8;    // Flags
pub const AT_ENTRY: u64 = 9;    // Entry point of program
pub const AT_NOTELF: u64 = 10;  // Program is not ELF
pub const AT_UID: u64 = 11;     // Real uid
pub const AT_EUID: u64 = 12;    // Effective uid
pub const AT_GID: u64 = 13;     // Real gid
pub const AT_EGID: u64 = 14;    // Effective gid
pub const AT_PLATFORM: u64 = 15; // String identifying platform
pub const AT_HWCAP: u64 = 16;   // Machine dependent hints about processor capabilities
pub const AT_CLKTCK: u64 = 17;  // Frequency of times()
pub const AT_RANDOM: u64 = 25;  // Address of 16 random bytes

/// Auxiliary Vector entry
#[derive(Debug, Clone, Copy)]
pub struct AuxVec {
    pub a_type: u64,
    pub a_val: u64,
}

impl AuxVec {
    pub fn new(a_type: u64, a_val: u64) -> Self {
        Self { a_type, a_val }
    }
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
    analyze_and_load_elf_with_strategy(file_obj, task, &LoadStrategy::default())
}

/// Analyze ELF file and load it with custom loading strategy
/// 
/// This function determines whether the ELF file requires dynamic linking by checking
/// for PT_INTERP segment, then loads either the interpreter (dynamic linker) or the
/// main program directly (static linking) using the provided strategy.
/// 
/// # Arguments
/// 
/// * `file_obj`: A reference to the file object containing the ELF data
/// * `task`: A mutable reference to the task into which the ELF file will be loaded
/// * `strategy`: Loading strategy provided by ABI module
/// 
/// # Returns
/// 
/// * `Result<LoadElfResult, ElfLoaderError>`: Information about the loaded ELF including
///   execution mode, entry point, and auxiliary vector data
/// 
pub fn analyze_and_load_elf_with_strategy(
    file_obj: &dyn FileObject, 
    task: &mut Task,
    strategy: &LoadStrategy
) -> Result<LoadElfResult, ElfLoaderError> {
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
    
    // Convert ELF type to format-agnostic information
    let needs_relocation = header.e_type == ET_DYN;
    
    match interpreter_path {
        Some(interp_path) => {
            // Dynamic linking required
            
            // Let strategy resolve the actual interpreter to use
            let actual_interpreter = (strategy.resolve_interpreter)(Some(&interp_path));
            
            if let Some(final_interp_path) = actual_interpreter {
                let base_address = load_elf_segments_for_interpreter(&header, file_obj, task, strategy)?;
                let interpreter_entry = load_interpreter(&final_interp_path, task, strategy)?;
                
                // Prepare program headers info for auxiliary vector
                let phdr_info = ProgramHeadersInfo {
                    phdr_addr: base_address + header.e_phoff,
                    phdr_size: header.e_phentsize as u64,
                    phdr_count: header.e_phnum as u64,
                };
                
                Ok(LoadElfResult {
                    mode: ExecutionMode::Dynamic { interpreter_path: final_interp_path },
                    entry_point: interpreter_entry,
                    base_address: Some(base_address),
                    program_headers: phdr_info,
                })
            } else {
                // Strategy rejected dynamic linking (e.g., xv6 ABI)
                return Err(ElfLoaderError {
                    message: "Dynamic linking not supported by current ABI".to_string(),
                });
            }
        }
        None => {
            // Static linking - use existing implementation
            let base_address = (strategy.choose_base_address)(LoadTarget::MainProgram, needs_relocation);
            let entry_point = load_elf_into_task_static(&header, file_obj, task, strategy)?;
            
            // For static executables, program headers are still available for debugging/profiling
            let phdr_info = ProgramHeadersInfo {
                phdr_addr: base_address + header.e_phoff,
                phdr_size: header.e_phentsize as u64,
                phdr_count: header.e_phnum as u64,
            };
            
            Ok(LoadElfResult {
                mode: ExecutionMode::Static,
                entry_point,
                base_address: if needs_relocation { Some(base_address) } else { None },
                program_headers: phdr_info,
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
fn load_elf_segments_for_interpreter(header: &ElfHeader, file_obj: &dyn FileObject, task: &mut Task, strategy: &LoadStrategy) -> Result<u64, ElfLoaderError> {
    // Use strategy to determine base address
    let needs_relocation = header.e_type == ET_DYN;
    let base_address = (strategy.choose_base_address)(LoadTarget::MainProgram, needs_relocation);
    
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
            let vaddr = segment_addr as usize;
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
                        message: format!("Failed to translate virtual address {:#x} for interpreter loading", vaddr),
                    });
                }
            }
            
            // Update task size information (for proper memory management)
            let segment_type = if ph.p_flags & PF_X != 0 {
                task.text_size += aligned_size;
                "text"
            } else if ph.p_flags & PF_W != 0 || ph.p_flags & PF_R != 0 {
                task.data_size += aligned_size;
                "data"
            } else {
                "unknown"
            };
            
            crate::println!("Loaded interpreter {} segment at {:#x} (size: {:#x})", segment_type, segment_addr, aligned_size);
        }
    }
    
    Ok(base_address)
}

/// Load interpreter (dynamic linker) into task memory  
/// Maximum recursion depth for interpreter loading to prevent infinite loops
const MAX_INTERPRETER_DEPTH: usize = 5;

fn load_interpreter(interpreter_path: &str, task: &mut Task, strategy: &LoadStrategy) -> Result<u64, ElfLoaderError> {
    load_interpreter_recursive(interpreter_path, task, strategy, 0)
}

/// Recursive interpreter loading with depth limiting
fn load_interpreter_recursive(interpreter_path: &str, task: &mut Task, strategy: &LoadStrategy, depth: usize) -> Result<u64, ElfLoaderError> {
    // Check recursion depth to prevent infinite loops
    if depth >= MAX_INTERPRETER_DEPTH {
        return Err(ElfLoaderError {
            message: format!("Maximum interpreter recursion depth ({}) exceeded", MAX_INTERPRETER_DEPTH),
        });
    }
    
    crate::println!("Loading interpreter (depth {}): {}", depth, interpreter_path);
    
    // Step 1: Open interpreter file from VFS
    let vfs = task.get_vfs().ok_or_else(|| ElfLoaderError {
        message: "Task VFS not available for interpreter loading".to_string(),
    })?;
    
    let file_obj = vfs.open(interpreter_path, 0).map_err(|fs_err| ElfLoaderError {
        message: format!("Failed to open interpreter '{}': {:?}", interpreter_path, fs_err),
    })?;
    
    // Extract FileObject from KernelObject and keep it alive
    let file_arc = match file_obj {
        crate::object::KernelObject::File(file_ref) => {
            file_ref
        },
        _ => return Err(ElfLoaderError {
            message: "Invalid kernel object type for interpreter file".to_string(),
        }),
    };
    
    let file_object: &dyn crate::fs::FileObject = file_arc.as_ref();
    
    // Step 2: Read ELF header data from file
    file_object.seek(crate::fs::SeekFrom::Start(0)).map_err(|e| ElfLoaderError {
        message: format!("Failed to seek to start of interpreter file: {:?}", e),
    })?;
    
    let mut header_buffer = vec![0u8; core::mem::size_of::<ElfHeader>()];
    file_object.read(&mut header_buffer).map_err(|e| ElfLoaderError {
        message: format!("Failed to read interpreter ELF header: {:?}", e),
    })?;
    
    let interp_header = ElfHeader::parse(&header_buffer).map_err(|e| ElfLoaderError {
        message: format!("Failed to parse interpreter ELF header: {}", e.message),
    })?;
    
    // Step 3: Check if this interpreter itself has an interpreter (recursive case)
    let nested_interpreter_path = find_interpreter_path(&interp_header, file_object)?;
    let final_entry_point = if let Some(nested_path) = nested_interpreter_path {
        let resolved_nested_path = (strategy.resolve_interpreter)(Some(&nested_path))
            .unwrap_or(nested_path);
        crate::println!("Interpreter {} requests nested interpreter: {}", interpreter_path, resolved_nested_path);
        
        // Recursively load the nested interpreter first
        load_interpreter_recursive(&resolved_nested_path, task, strategy, depth + 1)?
    } else {
        // No nested interpreter, load this interpreter normally
        let interp_needs_relocation = interp_header.e_type == ET_DYN;
        
        // Use strategy to determine base address for interpreter
        let interpreter_base = (strategy.choose_base_address)(LoadTarget::Interpreter, interp_needs_relocation);
        crate::println!("Interpreter base address: {:#x}", interpreter_base);
        
        // Load interpreter segments with specific base address
        load_elf_segments_with_base(&interp_header, file_object, task, interpreter_base)?;
        
        // Calculate actual entry point
        if interp_needs_relocation {
            interpreter_base + interp_header.e_entry as u64
        } else {
            interp_header.e_entry
        }
    };
    
    crate::println!("Interpreter entry point (depth {}): {:#x}", depth, final_entry_point);
    Ok(final_entry_point)
}

/// Load ELF segments for interpreter with specified base address
fn load_elf_segments_with_base(header: &ElfHeader, file_obj: &dyn FileObject, task: &mut Task, base_address: u64) -> Result<(), ElfLoaderError> {
    // Load PT_LOAD segments with provided base address
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
            // Map segment for interpreter
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
            let vaddr = segment_addr as usize;
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
                        message: format!("Failed to translate virtual address {:#x} for interpreter loading", vaddr),
                    });
                }
            }
            
            // Update task size information for proper memory management
            let segment_type = if ph.p_flags & PF_X != 0 {
                task.text_size += aligned_size;
                "text"
            } else if ph.p_flags & PF_W != 0 || ph.p_flags & PF_R != 0 {
                task.data_size += aligned_size;
                "data"
            } else {
                "unknown"
            };
            
            crate::println!("Loaded interpreter {} segment at {:#x} (size: {:#x})", segment_type, segment_addr, aligned_size);
        }
    }
    
    Ok(())
}

/// Load ELF using the static linking logic with strategy support
fn load_elf_into_task_static(header: &ElfHeader, file_obj: &dyn FileObject, task: &mut Task, strategy: &LoadStrategy) -> Result<u64, ElfLoaderError> {
    // Use strategy to determine base address for main program
    let needs_relocation = header.e_type == ET_DYN;
    let base_address = (strategy.choose_base_address)(LoadTarget::MainProgram, needs_relocation);
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
            // Calculate proper alignment-aware mapping with base address
            let segment_addr = base_address + ph.p_vaddr;
            let align = ph.p_align as usize;
            // Handle zero or invalid alignment (ELF spec: 0 or 1 means no alignment constraint)
            if align == 0 {
                return Err(ElfLoaderError {
                    message: format!("Invalid alignment value: segment has zero alignment requirement"),
                });
            }
            let page_offset = (segment_addr as usize) % align;
            let mapping_start = (segment_addr as usize) - page_offset;
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
            let vaddr = segment_addr as usize;
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

/// Build auxiliary vector for dynamic linking
pub fn build_auxiliary_vector(
    load_result: &LoadElfResult,
    interpreter_base: Option<u64>,
) -> alloc::vec::Vec<AuxVec> {
    use crate::environment::PAGE_SIZE;
    
    let mut auxv = alloc::vec::Vec::new();
    
    // Program headers information
    auxv.push(AuxVec::new(AT_PHDR, load_result.program_headers.phdr_addr));
    auxv.push(AuxVec::new(AT_PHENT, load_result.program_headers.phdr_size));
    auxv.push(AuxVec::new(AT_PHNUM, load_result.program_headers.phdr_count));
    
    // System information
    auxv.push(AuxVec::new(AT_PAGESZ, PAGE_SIZE as u64));
    
    // Entry point of main program
    if let Some(base) = load_result.base_address {
        auxv.push(AuxVec::new(AT_ENTRY, base));
    }
    
    // Base address of interpreter (if dynamically linked)
    if let Some(interp_base) = interpreter_base {
        auxv.push(AuxVec::new(AT_BASE, interp_base));
    }
    
    // TODO: Add more auxiliary vector entries as needed:
    // - AT_RANDOM: Random bytes for stack canaries
    // - AT_UID, AT_EUID, AT_GID, AT_EGID: User/group IDs
    // - AT_PLATFORM: Platform string
    // - AT_HWCAP: Hardware capabilities
    
    // Terminate auxiliary vector
    auxv.push(AuxVec::new(AT_NULL, 0));
    
    auxv
}

/// Setup auxiliary vector on the task's stack
/// 
/// This function places the auxiliary vector at the top of the stack,
/// which is expected by the dynamic linker and C runtime.
pub fn setup_auxiliary_vector_on_stack(
    task: &mut Task,
    auxv: &[AuxVec],
) -> Result<usize, ElfLoaderError> {
    // Calculate size needed for auxiliary vector
    // Each AuxVec entry is 16 bytes (two u64 values)
    let auxv_size = auxv.len() * core::mem::size_of::<AuxVec>();
    
    // Find the top of the stack
    let stack_top = crate::environment::USER_STACK_END;
    let auxv_start = stack_top - auxv_size;
    
    // Write auxiliary vector to stack
    for (i, entry) in auxv.iter().enumerate() {
        let offset = i * core::mem::size_of::<AuxVec>();
        let vaddr = auxv_start + offset;
        
        // Translate to physical address and write
        match task.vm_manager.translate_vaddr(vaddr) {
            Some(paddr) => {
                unsafe {
                    let ptr = paddr as *mut AuxVec;
                    ptr.write(*entry);
                }
            },
            None => {
                return Err(ElfLoaderError {
                    message: format!("Failed to translate auxiliary vector address {:#x}", vaddr),
                });
            }
        }
    }
    
    crate::println!("Setup auxiliary vector at {:#x} (size: {} entries)", auxv_start, auxv.len());
    Ok(auxv_start)
}

#[cfg(test)]
mod tests;