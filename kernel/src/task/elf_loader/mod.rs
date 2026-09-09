//! ELF Loading Module
//!
//! This module provides functionality for loading ELF (Executable and Linkable Format)
//! executables into a task's memory space. It decodes ELF32 and ELF64 headers for dynamic
//! linking capabilities and handles the parsing of ELF headers and program headers, as well
//! as the mapping of loadable segments into memory.
//!
//! # Components
//!
//! - `ElfHeader`: Represents the ELF file header which contains metadata about the file
//! - `ProgramHeader`: Represents a program header which describes a segment in the ELF file
//! - `LoadedSegment`: Represents a segment after it has been loaded into memory
//! - Dynamic linking support for shared libraries and position-independent executables
//! - Error types for handling various failure scenarios during ELF parsing and loading
//!
//! # Main Functions
//!
//! - `load_elf_into_task`: Loads an ELF file from a file object into a task's memory space
//! - `map_elf_segment`: Maps an ELF segment into a task's virtual memory
//! - Dynamic linker integration for shared library resolution
//!
//! # Dynamic Linking Support
//!
//! The module now includes comprehensive dynamic linking capabilities:
//! - Dynamic symbol resolution
//! - Shared library loading and linking
//! - Position-independent executable (PIE) support
//! - Runtime relocation handling
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
use crate::fs::{FileObject, SeekFrom};
use crate::task::Task;
use crate::vm::addr::{phys_to_virt, virt_to_phys};
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission, VirtualMemoryRegion};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::{format, vec};
use core::sync::atomic::Ordering;

use super::TaskType;

// ELF Magic Number
const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];
// ELF Class
const ELFCLASS32: u8 = 1; // 32-bit
const ELFCLASS64: u8 = 2; // 64-bit
// ELF Data Endian
const ELFDATA2LSB: u8 = 1; // Little Endian
// const ELFDATA2MSB: u8 = 2; // Big Endian

// ELF File Type
pub const ET_EXEC: u16 = 2; // Executable file
pub const ET_DYN: u16 = 3; // Shared object file / Position Independent Executable

// Program Header Type
const PT_LOAD: u32 = 1; // Loadable segment
const PT_INTERP: u32 = 3; // Interpreter path

/// Target type for ELF loading (determines base address strategy)
#[derive(Debug, Clone, Copy)]
pub enum LoadTarget {
    MainProgram, // Main executable being loaded
    Interpreter, // Dynamic linker/interpreter
    SharedLib,   // Shared library (future use)
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
                    (LoadTarget::MainProgram, false) => 0, // Absolute addresses
                    (LoadTarget::MainProgram, true) => 0x10000, // PIE executable
                    (LoadTarget::Interpreter, _) => 0x40000000, // Dynamic linker
                    (LoadTarget::SharedLib, _) => 0x50000000, // Shared libraries
                }
            },
            resolve_interpreter: |requested| requested.map(|s| s.to_string()),
        }
    }
}

/// Execution mode determined by ELF analysis
#[derive(Debug, Clone)]
pub enum ExecutionMode {
    /// Static linking - direct execution
    Static,
    /// Dynamic linking - needs interpreter
    Dynamic { interpreter_path: String },
}

/// Result of ELF loading analysis
#[derive(Debug, Clone)]
pub struct LoadElfResult {
    /// Execution mode (static or dynamic)
    pub mode: ExecutionMode,
    /// Entry point (either main program or interpreter)
    pub entry_point: u64,
    /// Original program entry point (for AT_ENTRY in dynamic linking)
    pub original_entry_point: Option<u64>,
    /// Base address where main program was loaded (for auxiliary vector)
    pub base_address: Option<u64>,
    /// Base address where interpreter was loaded (for AT_BASE)
    pub interpreter_base: Option<u64>,
    /// Program headers info (for auxiliary vector)
    pub program_headers: ProgramHeadersInfo,
}

/// Program headers information for auxiliary vector
#[derive(Debug, Clone)]
pub struct ProgramHeadersInfo {
    pub phdr_addr: u64,  // Address of program headers in memory
    pub phdr_size: u64,  // Size of program header entry
    pub phdr_count: u64, // Number of program headers
}

// Auxiliary Vector (auxv) types for dynamic linking
/// Auxiliary Vector entry type constants
pub const AT_NULL: u64 = 0; // End of vector
pub const AT_IGNORE: u64 = 1; // Entry should be ignored
pub const AT_EXECFD: u64 = 2; // File descriptor of program
pub const AT_PHDR: u64 = 3; // Program headers for program
pub const AT_PHENT: u64 = 4; // Size of program header entry
pub const AT_PHNUM: u64 = 5; // Number of program headers
pub const AT_PAGESZ: u64 = 6; // System page size
pub const AT_BASE: u64 = 7; // Base address of interpreter
pub const AT_FLAGS: u64 = 8; // Flags
pub const AT_ENTRY: u64 = 9; // Entry point of program
pub const AT_NOTELF: u64 = 10; // Program is not ELF
pub const AT_UID: u64 = 11; // Real uid
pub const AT_EUID: u64 = 12; // Effective uid
pub const AT_GID: u64 = 13; // Real gid
pub const AT_EGID: u64 = 14; // Effective gid
pub const AT_PLATFORM: u64 = 15; // String identifying platform
pub const AT_HWCAP: u64 = 16; // Machine dependent hints about processor capabilities
pub const AT_CLKTCK: u64 = 17; // Frequency of times()
pub const AT_RANDOM: u64 = 25; // Address of 16 random bytes

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
    let bytes = buffer[offset..offset + 2].try_into().unwrap();
    if is_little_endian {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    }
}

fn read_u32(buffer: &[u8], offset: usize, is_little_endian: bool) -> u32 {
    let bytes = buffer[offset..offset + 4].try_into().unwrap();
    if is_little_endian {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}

fn read_u64(buffer: &[u8], offset: usize, is_little_endian: bool) -> u64 {
    let bytes = buffer[offset..offset + 8].try_into().unwrap();
    if is_little_endian {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    }
}

#[derive(Debug)]
pub struct ElfHeader {
    pub ei_class: u8,     // 32-bit or 64-bit (EI_CLASS)
    pub ei_data: u8,      // Endianness (EI_DATA)
    pub e_type: u16,      // File type
    pub e_machine: u16,   // Machine type
    pub e_version: u32,   // ELF version
    pub e_entry: u64,     // Entry point address
    pub e_phoff: u64,     // Program header table file offset
    pub e_shoff: u64,     // Section header table file offset
    pub e_flags: u32,     // Processor-specific flags
    pub e_ehsize: u16,    // ELF header size
    pub e_phentsize: u16, // Program header table entry size
    pub e_phnum: u16,     // Number of program header entries
    pub e_shentsize: u16, // Section header table entry size
    pub e_shnum: u16,     // Number of section header entries
    pub e_shstrndx: u16,  // Section header string table index
}

#[derive(Debug)]
pub struct ProgramHeader {
    pub p_type: u32,   // Segment type
    pub p_flags: u32,  // Segment flags
    pub p_offset: u64, // Segment offset in file
    pub p_vaddr: u64,  // Segment virtual address for loading
    pub p_paddr: u64,  // Segment physical address (usually unused)
    pub p_filesz: u64, // Segment size in file
    pub p_memsz: u64,  // Segment size in memory
    pub p_align: u64,  // Segment alignment
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

fn elf_sizes(class: u8) -> Option<(usize, usize)> {
    match class {
        ELFCLASS32 => Some((52, 32)),
        ELFCLASS64 => Some((64, 56)),
        _ => None,
    }
}

impl ElfHeader {
    /// Decode the file's own class, independently of the host pointer width.
    pub fn parse(buffer: &[u8]) -> Result<Self, ElfHeaderParseError> {
        let invalid = |message: &str| ElfHeaderParseError {
            kind: ElfHeaderParseErrorKind::InvalidData,
            message: message.to_string(),
        };
        if buffer.len() < 16 {
            return Err(invalid("ELF identification is truncated"));
        }
        if buffer[..4] != ELFMAG {
            return Err(ElfHeaderParseError {
                kind: ElfHeaderParseErrorKind::InvalidMagicNumber,
                message: "Invalid ELF magic number".to_string(),
            });
        }
        let ei_class = buffer[EI_CLASS];
        let (header_size, ph_size) = elf_sizes(ei_class).ok_or_else(|| ElfHeaderParseError {
            kind: ElfHeaderParseErrorKind::UnsupportedClass,
            message: "Unknown ELF class".to_string(),
        })?;
        if buffer.len() < header_size {
            return Err(invalid("ELF header is truncated"));
        }
        let ei_data = buffer[EI_DATA];
        if !matches!(ei_data, 1 | 2) || buffer[6] != 1 {
            return Err(invalid("Invalid ELF encoding or identification version"));
        }
        let le = ei_data == ELFDATA2LSB;
        let (e_entry, e_phoff, e_shoff, tail) = match ei_class {
            ELFCLASS32 => (
                read_u32(buffer, 24, le) as u64,
                read_u32(buffer, 28, le) as u64,
                read_u32(buffer, 32, le) as u64,
                36,
            ),
            ELFCLASS64 => (
                read_u64(buffer, 24, le),
                read_u64(buffer, 32, le),
                read_u64(buffer, 40, le),
                48,
            ),
            _ => unreachable!(),
        };
        let header = Self {
            ei_class,
            ei_data,
            e_type: read_u16(buffer, 16, le),
            e_machine: read_u16(buffer, 18, le),
            e_version: read_u32(buffer, 20, le),
            e_entry,
            e_phoff,
            e_shoff,
            e_flags: read_u32(buffer, tail, le),
            e_ehsize: read_u16(buffer, tail + 4, le),
            e_phentsize: read_u16(buffer, tail + 6, le),
            e_phnum: read_u16(buffer, tail + 8, le),
            e_shentsize: read_u16(buffer, tail + 10, le),
            e_shnum: read_u16(buffer, tail + 12, le),
            e_shstrndx: read_u16(buffer, tail + 14, le),
        };
        if header.e_version != 1
            || header.e_ehsize as usize != header_size
            || (header.e_phnum != 0 && header.e_phentsize as usize != ph_size)
        {
            return Err(invalid("ELF version or header entry sizes are invalid"));
        }
        Ok(header)
    }

    fn validate_executable(&self) -> Result<(), ElfLoaderError> {
        let class = if usize::BITS == 32 {
            ELFCLASS32
        } else {
            ELFCLASS64
        };
        #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
        let machine = 243;
        #[cfg(target_arch = "aarch64")]
        let machine = 183;
        if self.ei_class != class || self.e_machine != machine || self.ei_data != ELFDATA2LSB {
            return Err(elf_error(
                "ELF class, machine or byte order does not match the execution architecture",
            ));
        }
        if !matches!(self.e_type, ET_EXEC | ET_DYN) || self.e_phnum == 0 || self.e_phnum == u16::MAX
        {
            return Err(elf_error(
                "ELF is not an executable with a supported program-header table",
            ));
        }
        self.e_phoff
            .checked_add(self.e_phnum as u64 * self.e_phentsize as u64)
            .ok_or_else(|| elf_error("ELF program-header table overflows"))?;
        Ok(())
    }
}

impl ProgramHeader {
    pub fn parse(buffer: &[u8], class: u8, le: bool) -> Result<Self, ProgramHeaderParseError> {
        let invalid = || ProgramHeaderParseError {
            kind: ProgramHeaderParseErrorKind::InvalidSize,
            message: "Unsupported or truncated program header".to_string(),
        };
        let (_, size) = elf_sizes(class).ok_or_else(invalid)?;
        if buffer.len() < size {
            return Err(invalid());
        }
        Ok(match class {
            ELFCLASS32 => Self {
                p_type: read_u32(buffer, 0, le),
                p_offset: read_u32(buffer, 4, le) as u64,
                p_vaddr: read_u32(buffer, 8, le) as u64,
                p_paddr: read_u32(buffer, 12, le) as u64,
                p_filesz: read_u32(buffer, 16, le) as u64,
                p_memsz: read_u32(buffer, 20, le) as u64,
                p_flags: read_u32(buffer, 24, le),
                p_align: read_u32(buffer, 28, le) as u64,
            },
            ELFCLASS64 => Self {
                p_type: read_u32(buffer, 0, le),
                p_flags: read_u32(buffer, 4, le),
                p_offset: read_u64(buffer, 8, le),
                p_vaddr: read_u64(buffer, 16, le),
                p_paddr: read_u64(buffer, 24, le),
                p_filesz: read_u64(buffer, 32, le),
                p_memsz: read_u64(buffer, 40, le),
                p_align: read_u64(buffer, 48, le),
            },
            _ => unreachable!(),
        })
    }
}

fn elf_error(message: &str) -> ElfLoaderError {
    ElfLoaderError {
        message: message.to_string(),
    }
}

fn read_exact(file: &dyn FileObject, mut buffer: &mut [u8]) -> Result<(), ElfLoaderError> {
    while !buffer.is_empty() {
        let read = file.read(buffer).map_err(|error| ElfLoaderError {
            message: format!("ELF read failed: {:?}", error),
        })?;
        if read == 0 || read > buffer.len() {
            return Err(elf_error("ELF file is truncated"));
        }
        buffer = &mut buffer[read..];
    }
    Ok(())
}

fn read_elf_header(file: &dyn FileObject) -> Result<ElfHeader, ElfLoaderError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| elf_error("ELF header seek failed"))?;
    let mut bytes = [0u8; 64];
    read_exact(file, &mut bytes[..16])?;
    let (size, _) = elf_sizes(bytes[EI_CLASS]).ok_or_else(|| elf_error("Unsupported ELF class"))?;
    read_exact(file, &mut bytes[16..size])?;
    let header = ElfHeader::parse(&bytes[..size]).map_err(|error| ElfLoaderError {
        message: error.message,
    })?;
    header.validate_executable()?;
    Ok(header)
}

/// Read and parse a program header at the specified index
fn read_program_header(
    header: &ElfHeader,
    file_obj: &dyn FileObject,
    index: u16,
) -> Result<ProgramHeader, ElfLoaderError> {
    if index >= header.e_phnum {
        return Err(elf_error("Program header index out of bounds"));
    }
    let offset = header
        .e_phoff
        .checked_add(index as u64 * header.e_phentsize as u64)
        .ok_or_else(|| elf_error("Program header offset overflows"))?;
    file_obj
        .seek(SeekFrom::Start(offset))
        .map_err(|e| ElfLoaderError {
            message: format!("Failed to seek to program header {}: {:?}", index, e),
        })?;

    let mut ph_buffer = vec![0u8; header.e_phentsize as usize];
    read_exact(file_obj, &mut ph_buffer)?;
    let ph = ProgramHeader::parse(&ph_buffer, header.ei_class, header.ei_data == ELFDATA2LSB)
        .map_err(|error| ElfLoaderError {
            message: error.message,
        })?;
    if ph.p_type == PT_LOAD {
        if ph.p_filesz > ph.p_memsz
            || (ph.p_align > 1
                && (!ph.p_align.is_power_of_two()
                    || ph.p_vaddr % ph.p_align != ph.p_offset % ph.p_align))
        {
            return Err(elf_error("Invalid ELF segment size or alignment"));
        }
        ph.p_vaddr
            .checked_add(ph.p_memsz)
            .ok_or_else(|| elf_error("ELF segment address overflows"))?;
    }
    ph.p_offset
        .checked_add(ph.p_filesz)
        .ok_or_else(|| elf_error("ELF segment file range overflows"))?;
    Ok(ph)
}

/// Iterate through all program headers and call a closure for each one
fn for_each_program_header<F>(
    header: &ElfHeader,
    file_obj: &dyn FileObject,
    mut callback: F,
) -> Result<(), ElfLoaderError>
where
    F: FnMut(u16, &ProgramHeader) -> Result<bool, ElfLoaderError>, // Return false to break early
{
    for i in 0..header.e_phnum {
        let ph = read_program_header(header, file_obj, i)?;
        let should_continue = callback(i, &ph)?;
        if !should_continue {
            break;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct LoadedSegment {
    pub vaddr: u64, // Virtual address
    pub size: u64,  // Size
    pub flags: u32, // Flags (R/W/X)
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
pub fn load_elf_into_task(file_obj: &dyn FileObject, task: &Task) -> Result<u64, ElfLoaderError> {
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
pub fn analyze_and_load_elf(
    file_obj: &dyn FileObject,
    task: &Task,
) -> Result<LoadElfResult, ElfLoaderError> {
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
    task: &Task,
    strategy: &LoadStrategy,
) -> Result<LoadElfResult, ElfLoaderError> {
    let header = read_elf_header(file_obj)?;

    // Step 1: Check for PT_INTERP segment
    let interpreter_path = find_interpreter_path(&header, file_obj)?;

    // Convert ELF type to format-agnostic information
    let needs_relocation = header.e_type == ET_DYN;

    match interpreter_path {
        Some(interp_path) => {
            // Dynamic linking required
            crate::println!(
                "ELF requires dynamic linking with interpreter: {}",
                interp_path
            );

            // Let strategy resolve the actual interpreter to use
            let actual_interpreter = (strategy.resolve_interpreter)(Some(&interp_path));

            if let Some(final_interp_path) = actual_interpreter {
                crate::println!("Using interpreter: {}", final_interp_path);
                let base_address =
                    load_elf_segments_for_interpreter(&header, file_obj, task, strategy)?;
                let (interpreter_entry, interpreter_base) =
                    load_interpreter(&final_interp_path, task, strategy)?;

                // Prepare program headers info for auxiliary vector
                let phdr_info = ProgramHeadersInfo {
                    phdr_addr: base_address + header.e_phoff,
                    phdr_size: header.e_phentsize as u64,
                    phdr_count: header.e_phnum as u64,
                };

                // Calculate original entry point correctly based on ELF type
                // For ET_EXEC: e_entry is an absolute address
                // For ET_DYN: e_entry is relative to base_address
                let original_entry = if needs_relocation {
                    base_address + header.e_entry
                } else {
                    header.e_entry
                };

                Ok(LoadElfResult {
                    mode: ExecutionMode::Dynamic {
                        interpreter_path: final_interp_path,
                    },
                    entry_point: interpreter_entry,
                    original_entry_point: Some(original_entry),
                    base_address: Some(base_address),
                    interpreter_base: Some(interpreter_base),
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
            let base_address =
                (strategy.choose_base_address)(LoadTarget::MainProgram, needs_relocation);
            let entry_point = load_elf_into_task_static(&header, file_obj, task, strategy)?;

            // For static executables, load program headers into memory if needed
            let phdr_info = if needs_relocation {
                // PIE static executable - program headers are loaded with the executable
                ProgramHeadersInfo {
                    phdr_addr: base_address + header.e_phoff,
                    phdr_size: header.e_phentsize as u64,
                    phdr_count: header.e_phnum as u64,
                }
            } else {
                // Traditional static executable - load program headers into memory
                let phdr_mem_addr = load_program_headers_into_memory(&header, file_obj, task)?;
                ProgramHeadersInfo {
                    phdr_addr: phdr_mem_addr,
                    phdr_size: header.e_phentsize as u64,
                    phdr_count: header.e_phnum as u64,
                }
            };

            Ok(LoadElfResult {
                mode: ExecutionMode::Static,
                entry_point,
                original_entry_point: None, // Same as entry_point for static executables
                base_address: if needs_relocation {
                    Some(base_address)
                } else {
                    None
                },
                interpreter_base: None, // No interpreter for static linking
                program_headers: phdr_info,
            })
        }
    }
}

/// Find PT_INTERP segment and extract interpreter path
fn find_interpreter_path(
    header: &ElfHeader,
    file_obj: &dyn FileObject,
) -> Result<Option<String>, ElfLoaderError> {
    let mut result = None;

    for_each_program_header(header, file_obj, |_i, ph| {
        if ph.p_type == PT_INTERP {
            // Read interpreter path
            file_obj
                .seek(SeekFrom::Start(ph.p_offset))
                .map_err(|e| ElfLoaderError {
                    message: format!("Failed to seek to interpreter path: {:?}", e),
                })?;

            let size = usize::try_from(ph.p_filesz)
                .map_err(|_| elf_error("ELF interpreter path exceeds pointer width"))?;
            if size == 0 || size > 4096 {
                return Err(elf_error("ELF interpreter path length is invalid"));
            }
            let mut interp_buffer = vec![0u8; size];
            file_obj
                .read(&mut interp_buffer)
                .map_err(|e| ElfLoaderError {
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

            result = Some(path);
            return Ok(false); // Break early
        }
        Ok(true) // Continue iteration
    })?;

    Ok(result)
}

/// Load ELF segments for dynamic execution (without executing)
fn load_elf_segments_for_interpreter(
    header: &ElfHeader,
    file_obj: &dyn FileObject,
    task: &Task,
    strategy: &LoadStrategy,
) -> Result<u64, ElfLoaderError> {
    // Use strategy to determine base address
    let needs_relocation = header.e_type == ET_DYN;
    // crate::println!("[ELF Loader] Main program: e_type={:#x}, needs_relocation={}, e_phoff={:#x}",
    //     header.e_type, needs_relocation, header.e_phoff);
    let base_address = (strategy.choose_base_address)(LoadTarget::MainProgram, needs_relocation);
    // crate::println!("[ELF Loader] Chosen base_address={:#x}", base_address);

    // Track the actual load address of the first LOAD segment for program headers
    let mut first_load_addr: Option<u64> = None;
    let mut _load_segment_count = 0;

    // Load PT_LOAD segments using simplified approach
    for_each_program_header(header, file_obj, |_i, ph| {
        if ph.p_type == PT_LOAD {
            let segment_addr = base_address
                .checked_add(ph.p_vaddr)
                .ok_or_else(|| elf_error("ELF relocation overflows"))?;
            // crate::println!("[ELF Loader] PT_LOAD[{}]: p_vaddr={:#x}, p_memsz={:#x}, p_filesz={:#x}, p_flags={:#x} -> load_addr={:#x}",
            //     i, ph.p_vaddr, ph.p_memsz, ph.p_filesz, ph.p_flags, segment_addr);
            if first_load_addr.is_none() {
                first_load_addr = Some(segment_addr);
                // crate::println!("[ELF Loader] First LOAD segment at {:#x}", segment_addr);
            }
            load_elf_segment_at_address(ph, file_obj, task, segment_addr)?;
            _load_segment_count += 1;
        }
        Ok(true) // Continue iteration
    })?;

    // crate::println!("[ELF Loader] Loaded {} PT_LOAD segments, e_entry={:#x}", _load_segment_count, header.e_entry);

    // Calculate phdr_addr based on actual load address
    // Program headers are typically in the first LOAD segment
    let actual_base = first_load_addr.unwrap_or(base_address);

    // Program headers are already loaded as part of the first LOAD segment
    // (which typically includes the ELF header and program headers)
    // No need to create a separate mapping - just return the address
    // crate::println!("[ELF Loader] Program headers at {:#x} (actual_base={:#x} + e_phoff={:#x})",
    //     actual_base + header.e_phoff, actual_base, header.e_phoff);

    // Return the actual base address where the first segment was loaded
    Ok(actual_base)
}

/// Load interpreter (dynamic linker) into task memory  
/// Maximum recursion depth for interpreter loading to prevent infinite loops
const MAX_INTERPRETER_DEPTH: usize = 5;

fn load_interpreter(
    interpreter_path: &str,
    task: &Task,
    strategy: &LoadStrategy,
) -> Result<(u64, u64), ElfLoaderError> {
    load_interpreter_recursive(interpreter_path, task, strategy, 0)
}

/// Recursive interpreter loading with depth limiting
fn load_interpreter_recursive(
    interpreter_path: &str,
    task: &Task,
    strategy: &LoadStrategy,
    depth: usize,
) -> Result<(u64, u64), ElfLoaderError> {
    // Check recursion depth to prevent infinite loops
    if depth >= MAX_INTERPRETER_DEPTH {
        return Err(ElfLoaderError {
            message: format!(
                "Maximum interpreter recursion depth ({}) exceeded",
                MAX_INTERPRETER_DEPTH
            ),
        });
    }

    crate::println!(
        "Loading interpreter (depth {}): {}",
        depth,
        interpreter_path
    );

    // Step 1: Open interpreter file from VFS
    let vfs = task.get_vfs().ok_or_else(|| ElfLoaderError {
        message: "Task VFS not available for interpreter loading".to_string(),
    })?;

    let file_obj = vfs
        .open(interpreter_path, 0)
        .map_err(|fs_err| ElfLoaderError {
            message: format!(
                "Failed to open interpreter '{}': {:?}",
                interpreter_path, fs_err
            ),
        })?;

    // Extract FileObject from KernelObject and keep it alive
    let file_arc = match file_obj {
        crate::object::KernelObject::File(file_ref) => file_ref,
        _ => {
            return Err(ElfLoaderError {
                message: "Invalid kernel object type for interpreter file".to_string(),
            });
        }
    };

    let file_object: &dyn crate::fs::FileObject = file_arc.as_ref();

    // Step 2: Read ELF header data from file
    file_object
        .seek(crate::fs::SeekFrom::Start(0))
        .map_err(|e| ElfLoaderError {
            message: format!("Failed to seek to start of interpreter file: {:?}", e),
        })?;

    // ELF header is always 64 bytes for 64-bit ELF files
    let interp_header = read_elf_header(file_object)?;

    // Step 3: Check if this interpreter itself has an interpreter (recursive case)
    let nested_interpreter_path = find_interpreter_path(&interp_header, file_object)?;
    let (final_entry_point, final_base) = if let Some(nested_path) = nested_interpreter_path {
        let resolved_nested_path =
            (strategy.resolve_interpreter)(Some(&nested_path)).unwrap_or(nested_path);
        crate::println!(
            "Interpreter {} requests nested interpreter: {}",
            interpreter_path,
            resolved_nested_path
        );

        // Recursively load the nested interpreter first
        load_interpreter_recursive(&resolved_nested_path, task, strategy, depth + 1)?
    } else {
        // No nested interpreter, load this interpreter normally
        let interp_needs_relocation = interp_header.e_type == ET_DYN;

        // Determine total span of PT_LOAD segments to avoid overlap
        let mut min_vaddr: u64 = u64::MAX;
        let mut max_end: u64 = 0;
        for_each_program_header(&interp_header, file_object, |_i, ph| {
            if ph.p_type == PT_LOAD {
                if ph.p_vaddr < min_vaddr {
                    min_vaddr = ph.p_vaddr;
                }
                let end = ph.p_vaddr.saturating_add(ph.p_memsz);
                if end > max_end {
                    max_end = end;
                }
            }
            Ok(true)
        })?;

        if min_vaddr == u64::MAX {
            return Err(ElfLoaderError {
                message: "Interpreter has no PT_LOAD segments".to_string(),
            });
        }

        let span = usize::try_from(
            max_end
                .checked_sub(min_vaddr)
                .ok_or_else(|| elf_error("ELF interpreter range reversed"))?,
        )
        .map_err(|_| elf_error("ELF interpreter range exceeds pointer width"))?;
        let align = crate::environment::PAGE_SIZE;
        let span_aligned = span
            .checked_add(align - 1)
            .ok_or_else(|| elf_error("ELF interpreter pages overflow"))?
            & !(align - 1);

        // Prefer the strategy's hint, but pick an actually free area in the task's VM
        let _preferred =
            (strategy.choose_base_address)(LoadTarget::Interpreter, interp_needs_relocation);
        let start = task
            .vm_manager
            .find_unmapped_area(span_aligned, align)
            .ok_or_else(|| ElfLoaderError {
                message: "No unmapped area available for interpreter".to_string(),
            })? as u64;

        // Compute additive base so that the lowest PT_LOAD maps to `start`
        let interpreter_base_add = start.saturating_sub(min_vaddr);
        crate::println!(
            "Interpreter base address: {:#x} (mapped span: {:#x} bytes)",
            interpreter_base_add,
            span_aligned
        );

        // Load interpreter segments with this base
        load_elf_segments_with_base(&interp_header, file_object, task, interpreter_base_add)?;

        // Calculate actual entry point and return base used for relocations/AT_BASE
        let entry = if interp_needs_relocation {
            interpreter_base_add + interp_header.e_entry as u64
        } else {
            interp_header.e_entry
        };
        (entry, interpreter_base_add)
    };

    crate::println!(
        "Interpreter entry point (depth {}): {:#x}",
        depth,
        final_entry_point
    );
    Ok((final_entry_point, final_base))
}

/// Load ELF segments for interpreter with specified base address
fn load_elf_segments_with_base(
    header: &ElfHeader,
    file_obj: &dyn FileObject,
    task: &Task,
    base_address: u64,
) -> Result<(), ElfLoaderError> {
    // Load PT_LOAD segments with provided base address
    for_each_program_header(header, file_obj, |_i, ph| {
        if ph.p_type == PT_LOAD {
            let segment_addr = base_address
                .checked_add(ph.p_vaddr)
                .ok_or_else(|| elf_error("ELF relocation overflows"))?;
            load_elf_segment_at_address(ph, file_obj, task, segment_addr)?;
        }
        Ok(true) // Continue iteration
    })?;

    Ok(())
}

/// Load ELF using the static linking logic with strategy support
fn load_elf_into_task_static(
    header: &ElfHeader,
    file_obj: &dyn FileObject,
    task: &Task,
    strategy: &LoadStrategy,
) -> Result<u64, ElfLoaderError> {
    let needs_relocation = header.e_type == ET_DYN;
    let base = (strategy.choose_base_address)(LoadTarget::MainProgram, needs_relocation);
    for_each_program_header(header, file_obj, |_i, ph| {
        if ph.p_type == PT_LOAD {
            let address = base
                .checked_add(ph.p_vaddr)
                .ok_or_else(|| elf_error("ELF relocation overflows"))?;
            load_elf_segment_at_address(ph, file_obj, task, address)?;
        }
        Ok(true)
    })?;
    let entry = if needs_relocation {
        base.checked_add(header.e_entry)
            .ok_or_else(|| elf_error("ELF entry relocation overflows"))?
    } else {
        header.e_entry
    };
    let address =
        usize::try_from(entry).map_err(|_| elf_error("ELF entry exceeds pointer width"))?;
    let mut executable_entry = false;
    for_each_program_header(header, file_obj, |_i, ph| {
        if ph.p_type == PT_LOAD && ph.p_flags & PF_X != 0 {
            let start = base
                .checked_add(ph.p_vaddr)
                .ok_or_else(|| elf_error("ELF relocation overflows"))?;
            let end = start
                .checked_add(ph.p_memsz)
                .ok_or_else(|| elf_error("ELF segment end overflows"))?;
            executable_entry |= start <= entry && entry < end;
        }
        Ok(true)
    })?;
    if !executable_entry || task.vm_manager.translate_to_kva(address).is_none() {
        return Err(elf_error("ELF entry is outside loaded executable segments"));
    }
    Ok(entry)
}

/// Load program headers into task memory for static executables
///
/// This function allocates memory space for program headers and copies them
/// from the ELF file, returning the virtual address where they are loaded.
/// This is needed for static executables where program headers are not
/// automatically loaded as part of any segment.
///
/// # Arguments
///
/// * `header`: The parsed ELF header containing program header information
/// * `file_obj`: The file object to read program header data from
/// * `task`: The task to load program headers into
///
/// # Returns
///
/// * `Result<u64, ElfLoaderError>`: Virtual address where program headers are loaded
///
fn load_program_headers_into_memory(
    header: &ElfHeader,
    file_obj: &dyn FileObject,
    task: &Task,
) -> Result<u64, ElfLoaderError> {
    // Calculate total size of program headers
    let phdr_table_size = (header.e_phentsize as u64) * (header.e_phnum as u64);

    if phdr_table_size == 0 {
        return Err(ElfLoaderError {
            message: "No program headers to load".to_string(),
        });
    }

    // Find a suitable virtual address for program headers
    // Place them after the highest loaded segment to avoid conflicts
    // For simplicity, use a fixed address in the upper memory region
    let phdr_vaddr = 0x70000000u64; // 1.75GB - safe region for program headers

    // Calculate page-aligned size
    let page_aligned_size = ((phdr_table_size as usize) + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Map memory for program headers (read-only for security)
    map_elf_segment(
        task,
        phdr_vaddr as usize,
        page_aligned_size,
        PAGE_SIZE,
        PF_R,
    )
    .map_err(|e| ElfLoaderError {
        message: format!("Failed to map memory for program headers: {}", e),
    })?;

    // Read program headers from file
    file_obj
        .seek(SeekFrom::Start(header.e_phoff))
        .map_err(|e| ElfLoaderError {
            message: format!("Failed to seek to program headers: {:?}", e),
        })?;

    let mut phdr_data = vec![0u8; phdr_table_size as usize];
    file_obj.read(&mut phdr_data).map_err(|e| ElfLoaderError {
        message: format!("Failed to read program headers: {:?}", e),
    })?;

    // Copy program headers to task memory
    match task.vm_manager.translate_to_kva(phdr_vaddr as usize) {
        Some(kaddr) => unsafe {
            core::ptr::copy_nonoverlapping(
                phdr_data.as_ptr(),
                kaddr as *mut u8,
                phdr_table_size as usize,
            );
        },
        None => {
            return Err(ElfLoaderError {
                message: format!(
                    "Failed to translate program headers virtual address {:#x}",
                    phdr_vaddr
                ),
            });
        }
    }
    Ok(phdr_vaddr)
}

fn map_elf_segment(
    task: &Task,
    vaddr: usize,
    size: usize,
    align: usize,
    flags: u32,
) -> Result<(), &'static str> {
    // Ensure alignment is greater than zero
    if align == 0 {
        return Err("Alignment must be greater than zero");
    }

    // Ensure alignment is a power of 2 and at least PAGE_SIZE
    if !align.is_power_of_two() || align < PAGE_SIZE {
        return Err("Invalid alignment: must be power of 2 and at least PAGE_SIZE");
    }

    // Check if the size is valid (must be page-aligned for memory mapping)
    if size == 0 || size % PAGE_SIZE != 0 {
        return Err("Invalid size: must be non-zero and page-aligned");
    }

    // Check if the address is page-aligned (required for memory mapping)
    if vaddr % PAGE_SIZE != 0 {
        return Err("Address is not aligned to PAGE_SIZE");
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
    let end = vaddr.checked_add(size - 1).ok_or("ELF mapping overflows")?;
    if end >= crate::environment::USER_LOWER_CANONICAL_END {
        return Err("ELF mapping exceeds user address space");
    }
    let vmarea = MemoryArea { start: vaddr, end };

    // Check if the area is overlapping with existing mappings
    if let Some(_existing) = task.vm_manager.search_memory_map(vaddr) {
        // crate::println!("[ELF Loader] ERROR: Memory area {:#x}-{:#x} overlaps with existing mapping {:#x}-{:#x}",
        //     vaddr, vaddr + size - 1, existing.vmarea.start, existing.vmarea.end);
        return Err("Memory area overlaps with existing mapping");
    }

    let num_of_pages = size / PAGE_SIZE;
    let page_alloc =
        crate::mem::page::ContiguousPages::new(num_of_pages).ok_or("Failed to allocate memory")?;
    let ptr = page_alloc.as_ptr() as *mut u8;
    let pm_start = virt_to_phys(ptr as usize);
    let pmarea = crate::vm::vmem::PhysicalMemoryArea {
        start: pm_start,
        end: pm_start + size as u64 - 1,
    };

    let map = VirtualMemoryMap {
        vmarea,
        pmarea,
        vm_start: vmarea.start,
        permissions,
        is_shared: false,
        memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
        owner: None,
    };

    if let Err(e) = task.vm_manager.add_memory_map(map) {
        return Err(e);
    }

    task.page_allocations.write().push(page_alloc);

    Ok(())
}

/// Build auxiliary vector for dynamic linking
pub fn build_auxiliary_vector(load_result: &LoadElfResult) -> alloc::vec::Vec<AuxVec> {
    use crate::environment::PAGE_SIZE;

    let mut auxv = alloc::vec::Vec::new();

    // Program headers information
    auxv.push(AuxVec::new(AT_PHDR, load_result.program_headers.phdr_addr));
    auxv.push(AuxVec::new(AT_PHENT, load_result.program_headers.phdr_size));
    auxv.push(AuxVec::new(
        AT_PHNUM,
        load_result.program_headers.phdr_count,
    ));

    // System information
    auxv.push(AuxVec::new(AT_PAGESZ, PAGE_SIZE as u64));

    // Entry point of main program (not the interpreter)
    // For dynamic executables, AT_ENTRY should be the original program's entry point
    match &load_result.mode {
        ExecutionMode::Dynamic { .. } => {
            // For dynamic executables, use the original program's entry point
            if let Some(orig_entry) = load_result.original_entry_point {
                auxv.push(AuxVec::new(AT_ENTRY, orig_entry));
            }
        }
        ExecutionMode::Static => {
            // For static executables, entry point is load_result.entry_point
            auxv.push(AuxVec::new(AT_ENTRY, load_result.entry_point));
        }
    }

    // Base address of interpreter (if dynamically linked)
    if let Some(interp_base) = load_result.interpreter_base {
        auxv.push(AuxVec::new(AT_BASE, interp_base));
    }

    // Add UID/GID entries to prevent musl secure mode
    // Set all IDs to 0 (root) to make real and effective IDs equal
    // This prevents libc.secure from being set to true
    auxv.push(AuxVec::new(AT_UID, 0)); // Real user ID
    auxv.push(AuxVec::new(AT_EUID, 0)); // Effective user ID
    auxv.push(AuxVec::new(AT_GID, 0)); // Real group ID
    auxv.push(AuxVec::new(AT_EGID, 0)); // Effective group ID

    // TODO: Add more auxiliary vector entries as needed:
    // - AT_RANDOM: Random bytes for stack canaries
    // - AT_PLATFORM: Platform string
    // - AT_HWCAP: Hardware capabilities

    // Terminate auxiliary vector
    auxv.push(AuxVec::new(AT_NULL, 0));

    auxv
}

/// Encode the selected ABI's word pairs, without exposing the in-kernel
/// representation or silently narrowing addresses in ELF32 auxiliary entries.
fn encode_auxiliary_vector(
    auxv: &[AuxVec],
    model: scarlet_abi::data_model::AbiDataModel,
) -> Result<alloc::vec::Vec<u8>, ElfLoaderError> {
    let word = model.word_width.bytes();
    let size = auxv
        .len()
        .checked_mul(2 * word)
        .ok_or_else(|| elf_error("Auxiliary vector size overflows"))?;
    let mut bytes = vec![0; size];
    for (index, entry) in auxv.iter().enumerate() {
        model
            .write_word(&mut bytes, index * 2 * word, entry.a_type)
            .and_then(|_| model.write_word(&mut bytes, (index * 2 + 1) * word, entry.a_val))
            .map_err(|_| elf_error("Auxiliary vector value exceeds ABI word width"))?;
    }
    Ok(bytes)
}

/// Construct one native process-start stack: argc, argv, NULL, envp, NULL,
/// auxv, then string storage. Metadata always uses native words and the stack
/// pointer is aligned to 16 bytes. RISC-V and AArch64 share this layout.
pub(crate) fn setup_native_stack(
    task: &Task,
    argv: &[&str],
    envp: &[&str],
    top: usize,
    auxv: &[AuxVec],
) -> Result<(usize, usize), &'static str> {
    let model = scarlet_abi::data_model::AbiDataModel::NATIVE;
    let word = model.word_width.bytes();
    let aux_bytes =
        encode_auxiliary_vector(auxv, model).map_err(|_| "Invalid native auxiliary vector")?;
    let strings_size = argv
        .iter()
        .chain(envp)
        .try_fold(0usize, |size, string| {
            if string.as_bytes().contains(&0) {
                return None;
            }
            size.checked_add(string.len())?.checked_add(1)
        })
        .ok_or("Invalid or oversized process arguments")?;
    let pointer_bytes = argv
        .len()
        .checked_add(envp.len())
        .and_then(|n| n.checked_add(3))
        .and_then(|n| n.checked_mul(word))
        .ok_or("Process pointer arrays overflow")?;
    let metadata_size = pointer_bytes
        .checked_add(aux_bytes.len())
        .ok_or("Process metadata overflows")?;
    let total = metadata_size
        .checked_add(strings_size)
        .ok_or("Process stack size overflows")?;
    let start = top.checked_sub(total).ok_or("Process stack underflows")? & !15;
    let stack_map = task
        .vm_manager
        .search_memory_map(top.checked_sub(1).ok_or("Empty process stack")?)
        .ok_or("Process stack is not mapped")?;
    if start < stack_map.vmarea.start {
        return Err("Process arguments exceed stack mapping");
    }
    let mut bytes = vec![0; top - start];
    model
        .write_word(&mut bytes, 0, argv.len() as u64)
        .map_err(|_| "Invalid argument count")?;
    let mut pointer = word;
    let mut string_offset = metadata_size;
    for strings in [argv, envp] {
        for string in strings {
            model
                .write_word(&mut bytes, pointer, (start + string_offset) as u64)
                .map_err(|_| "Invalid process string address")?;
            bytes[string_offset..string_offset + string.len()].copy_from_slice(string.as_bytes());
            string_offset += string.len() + 1;
            pointer += word;
        }
        pointer += word; // Zero-filled argv/envp terminator.
    }
    bytes[pointer..pointer + aux_bytes.len()].copy_from_slice(&aux_bytes);
    crate::library::std::usercopy::copy_to_user(task, start, &bytes)
        .map_err(|_| "Cannot write process stack")?;
    Ok((start, start + word))
}

#[cfg(test)]
mod tests;

/// Load a single ELF segment into task memory at the specified address
fn load_elf_segment_at_address(
    ph: &ProgramHeader,
    file_obj: &dyn FileObject,
    task: &Task,
    segment_addr: u64,
) -> Result<(), ElfLoaderError> {
    if ph.p_memsz == 0 {
        return Ok(());
    }
    let segment = usize::try_from(segment_addr)
        .map_err(|_| elf_error("ELF segment exceeds pointer width"))?;
    let size = usize::try_from(ph.p_memsz)
        .map_err(|_| elf_error("ELF segment size exceeds pointer width"))?;
    let file_size = usize::try_from(ph.p_filesz)
        .map_err(|_| elf_error("ELF file size exceeds pointer width"))?;
    if file_size > size {
        return Err(elf_error("ELF file data exceeds segment memory"));
    }
    let align = usize::try_from(ph.p_align)
        .map_err(|_| elf_error("ELF alignment exceeds pointer width"))?
        .max(PAGE_SIZE);
    let page_offset = segment % PAGE_SIZE;
    let mapping_start = segment - page_offset;
    let aligned_size = size
        .checked_add(page_offset)
        .and_then(|size| size.checked_add(PAGE_SIZE - 1))
        .map(|size| size & !(PAGE_SIZE - 1))
        .ok_or_else(|| elf_error("ELF page extent overflows"))?;
    let mapping_end = mapping_start
        .checked_add(aligned_size)
        .ok_or_else(|| elf_error("ELF mapping end overflows"))?;
    if mapping_end > crate::environment::USER_LOWER_CANONICAL_END {
        return Err(elf_error("ELF mapping exceeds user address space"));
    }

    // Map segment with proper page alignment
    map_elf_segment(task, mapping_start, aligned_size, align, ph.p_flags).map_err(|e| {
        ElfLoaderError {
            message: format!("Failed to map ELF segment at {:#x}: {:?}", mapping_start, e),
        }
    })?;

    // Copy file data to memory if there's any
    if ph.p_filesz > 0 {
        let mut segment_data = vec![0u8; file_size];
        file_obj
            .seek(SeekFrom::Start(ph.p_offset))
            .map_err(|e| ElfLoaderError {
                message: format!("Failed to seek to segment data: {:?}", e),
            })?;
        read_exact(file_obj, &mut segment_data)?;

        // Write data to task memory at the correct offset within the mapped region
        let data_offset = segment - mapping_start;
        let target_vaddr = mapping_start + data_offset;

        match task.vm_manager.translate_to_kva(target_vaddr) {
            Some(paddr) => unsafe {
                core::ptr::copy_nonoverlapping(segment_data.as_ptr(), paddr as *mut u8, file_size);
                if ph.p_flags & PF_X != 0 {
                    crate::arch::sync_icache_for_execution(paddr, file_size);
                }
            },
            None => {
                return Err(ElfLoaderError {
                    message: format!(
                        "Failed to translate virtual address {:#x} for segment loading",
                        target_vaddr
                    ),
                });
            }
        }
    }

    let old_brk = task.brk.load(Ordering::Relaxed);
    if old_brk == usize::MAX || old_brk < mapping_end {
        task.brk.store(mapping_end, Ordering::Relaxed);
    }

    // Update task size information for proper memory management
    let segment_type = if ph.p_flags & PF_X != 0 {
        task.text_size.fetch_add(aligned_size, Ordering::SeqCst);
        "text"
    } else if ph.p_flags & PF_W != 0 || ph.p_flags & PF_R != 0 {
        task.data_size.fetch_add(aligned_size, Ordering::SeqCst);
        "data"
    } else {
        "unknown"
    };

    crate::println!(
        "Loaded {} segment at {:#x} (size: {:#x})",
        segment_type,
        segment_addr,
        aligned_size
    );
    Ok(())
}
