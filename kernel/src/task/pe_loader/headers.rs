//! PE/COFF header definitions for loading Windows ARM64 binaries.
//!
//! This module provides structure definitions for PE32+ (PE64) format,
//! focusing on ARM64 executables (Machine type 0xAA64).
//!
//! All structures are `#[repr(C)]` and use fixed-size types for direct
//! parsing from file data without allocation.

/// ARM64 Little-Endian (Classic ABI)
pub const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;
/// ARM64EC for x64 interoperability
pub const IMAGE_FILE_MACHINE_ARM64EC: u16 = 0xA641;
/// ARM64X — fat binary with both ARM64 and ARM64EC
pub const IMAGE_FILE_MACHINE_ARM64X: u16 = 0xA64E;
/// AMD64 (x86_64) — rejected on load
pub const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
/// i386 — rejected on load
pub const IMAGE_FILE_MACHINE_I386: u16 = 0x014C;

pub const DOS_MAGIC: u16 = 0x5A4D;
pub const PE_SIGNATURE: u32 = 0x0000_4550;
/// PE32+ (64-bit) optional header magic
pub const PE32PLUS_MAGIC: u16 = 0x020B;
/// PE32 (32-bit) optional header magic
pub const PE32_MAGIC: u16 = 0x010B;

pub const IMAGE_SCN_CNT_CODE: u32 = 0x0000_0020;
pub const IMAGE_SCN_CNT_INITIALIZED_DATA: u32 = 0x0000_0040;
pub const IMAGE_SCN_CNT_UNINITIALIZED_DATA: u32 = 0x0000_0080;
pub const IMAGE_SCN_MEM_DISCARDABLE: u32 = 0x0200_0000;
pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
pub const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

pub const IMAGE_SUBSYSTEM_UNKNOWN: u16 = 0;
pub const IMAGE_SUBSYSTEM_NATIVE: u16 = 1;
pub const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;
pub const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;
pub const IMAGE_SUBSYSTEM_OS2_CUI: u16 = 5;
pub const IMAGE_SUBSYSTEM_POSIX_CUI: u16 = 7;

pub const IMAGE_DLLCHARACTERISTICS_HIGH_ENTROPY_VA: u16 = 0x0020;
pub const IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE: u16 = 0x0040;
pub const IMAGE_DLLCHARACTERISTICS_FORCE_INTEGRITY: u16 = 0x0080;
pub const IMAGE_DLLCHARACTERISTICS_NX_COMPAT: u16 = 0x0100;
pub const IMAGE_DLLCHARACTERISTICS_NO_ISOLATION: u16 = 0x0200;
pub const IMAGE_DLLCHARACTERISTICS_NO_SEH: u16 = 0x0400;
pub const IMAGE_DLLCHARACTERISTICS_NO_BIND: u16 = 0x0800;
pub const IMAGE_DLLCHARACTERISTICS_APPCONTAINER: u16 = 0x1000;
pub const IMAGE_DLLCHARACTERISTICS_WDM_DRIVER: u16 = 0x2000;
pub const IMAGE_DLLCHARACTERISTICS_GUARD_CF: u16 = 0x4000;
pub const IMAGE_DLLCHARACTERISTICS_TERMINAL_SERVER_AWARE: u16 = 0x8000;

pub const IMAGE_DIRECTORY_ENTRY_EXPORT: usize = 0;
pub const IMAGE_DIRECTORY_ENTRY_IMPORT: usize = 1;
pub const IMAGE_DIRECTORY_ENTRY_RESOURCE: usize = 2;
pub const IMAGE_DIRECTORY_ENTRY_EXCEPTION: usize = 3;
pub const IMAGE_DIRECTORY_ENTRY_SECURITY: usize = 4;
pub const IMAGE_DIRECTORY_ENTRY_BASERELOC: usize = 5;
pub const IMAGE_DIRECTORY_ENTRY_DEBUG: usize = 6;
pub const IMAGE_DIRECTORY_ENTRY_ARCHITECTURE: usize = 7;
pub const IMAGE_DIRECTORY_ENTRY_GLOBALPTR: usize = 8;
pub const IMAGE_DIRECTORY_ENTRY_TLS: usize = 9;
pub const IMAGE_DIRECTORY_ENTRY_LOAD_CONFIG: usize = 10;
pub const IMAGE_DIRECTORY_ENTRY_BOUND_IMPORT: usize = 11;
pub const IMAGE_DIRECTORY_ENTRY_IAT: usize = 12;
pub const IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT: usize = 13;
pub const IMAGE_DIRECTORY_ENTRY_CLR_RUNTIME: usize = 14;
pub const IMAGE_DIRECTORY_ENTRY_RESERVED: usize = 15;

pub const IMAGE_REL_ARM64_ABSOLUTE: u16 = 0x0000;
pub const IMAGE_REL_ARM64_ADDR32: u16 = 0x0001;
pub const IMAGE_REL_ARM64_ADDR32NB: u16 = 0x0002;
pub const IMAGE_REL_ARM64_BRANCH26: u16 = 0x0003;
pub const IMAGE_REL_ARM64_PAGEBASE_REL21: u16 = 0x0004;
pub const IMAGE_REL_ARM64_REL21: u16 = 0x0005;
pub const IMAGE_REL_ARM64_PAGEOFFSET_12A: u16 = 0x0006;
pub const IMAGE_REL_ARM64_PAGEOFFSET_12L: u16 = 0x0007;
pub const IMAGE_REL_ARM64_SECREL: u16 = 0x0008;
pub const IMAGE_REL_ARM64_SECREL_LOW12A: u16 = 0x0009;
pub const IMAGE_REL_ARM64_SECREL_HIGH12A: u16 = 0x000A;
pub const IMAGE_REL_ARM64_SECREL_LOW12L: u16 = 0x000B;
pub const IMAGE_REL_ARM64_TOKEN: u16 = 0x000C;
pub const IMAGE_REL_ARM64_SECTION: u16 = 0x000D;
pub const IMAGE_REL_ARM64_ADDR64: u16 = 0x000E;
pub const IMAGE_REL_ARM64_BRANCH19: u16 = 0x000F;
pub const IMAGE_REL_ARM64_BRANCH14: u16 = 0x0010;
pub const IMAGE_REL_ARM64_REL32: u16 = 0x0011;

/// DOS header — only the fields we need for PE navigation.
///
/// Layout (first 64 bytes of a PE file):
/// - Offset 0x00: e_magic (2 bytes)
/// - Offset 0x3C: e_lfanew (4 bytes) — pointer to PE header
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct DosHeader {
    pub e_magic: u16,
    // Skip to e_lfanew at offset 0x3C
}

impl DosHeader {
    /// Offset of the `e_lfanew` field from the start of the file.
    pub const LFANEW_OFFSET: usize = 0x3C;
}

/// PE signature — always 0x00004550 ("PE\0\0").
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct PeSignature {
    pub signature: u32,
}

/// COFF file header (20 bytes).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ImageFileHeader {
    pub machine: u16,
    pub number_of_sections: u16,
    pub time_date_stamp: u32,
    pub pointer_to_symbol_table: u32,
    pub number_of_symbols: u32,
    pub size_of_optional_header: u16,
    pub characteristics: u16,
}

impl ImageFileHeader {
    pub const SIZE: usize = 20;
}

/// PE32+ optional header — only the fields needed for loading.
///
/// Full structure is 240 bytes for PE32+. We define only the fields
/// we read, and compute offsets for the rest manually.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ImageOptionalHeader64 {
    pub magic: u16,
    pub major_linker_version: u8,
    pub minor_linker_version: u8,
    pub size_of_code: u32,
    pub size_of_initialized_data: u32,
    pub size_of_uninitialized_data: u32,
    pub address_of_entry_point: u32,
    pub base_of_code: u32,
    /// Image base (8 bytes on PE32+)
    pub image_base: u64,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub major_operating_system_version: u16,
    pub minor_operating_system_version: u16,
    pub major_image_version: u16,
    pub minor_image_version: u16,
    pub major_subsystem_version: u16,
    pub minor_subsystem_version: u16,
    pub win32_version_value: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub check_sum: u32,
    pub subsystem: u16,
    pub dll_characteristics: u16,
    pub size_of_stack_reserve: u64,
    pub size_of_stack_commit: u64,
    pub size_of_heap_reserve: u64,
    pub size_of_heap_commit: u64,
    pub loader_flags: u32,
    pub number_of_rva_and_sizes: u32,
    // Data directory entries follow immediately after this field.
}

impl ImageOptionalHeader64 {
    pub const SIZE: usize = 240;

    /// Offset of the `number_of_rva_and_sizes` field from the start of the optional header.
    pub const RVA_AND_SIZES_OFFSET: usize = 108;
}

/// A single data directory entry (8 bytes: RVA + Size).
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct ImageDataDirectory {
    pub virtual_address: u32, // RVA
    pub size: u32,
}

impl ImageDataDirectory {
    pub const SIZE: usize = 8;

    /// Returns true if this directory entry is present (non-zero RVA and size).
    pub fn is_present(&self) -> bool {
        self.virtual_address != 0 && self.size != 0
    }
}

/// Section header (40 bytes).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ImageSectionHeader {
    pub name: [u8; 8],
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub size_of_raw_data: u32,
    pub pointer_to_raw_data: u32,
    pub pointer_to_relocations: u32,
    pub pointer_to_linenumbers: u32,
    pub number_of_relocations: u16,
    pub number_of_linenumbers: u16,
    pub characteristics: u32,
}

impl ImageSectionHeader {
    pub const SIZE: usize = 40;

    /// Returns the section name as a string (truncated at first null byte).
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(8);
        // SAFETY: section names are ASCII in valid PE files
        core::str::from_utf8(&self.name[..end]).unwrap_or("?")
    }

    /// Returns true if this section contains executable code.
    pub fn is_executable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_EXECUTE != 0
    }

    /// Returns true if this section is readable.
    pub fn is_readable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_READ != 0
    }

    /// Returns true if this section is writable.
    pub fn is_writable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_WRITE != 0
    }

    /// Returns true if this section is discardable.
    pub fn is_discardable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_DISCARDABLE != 0
    }
}

/// Import descriptor (20 bytes). Null-terminated array.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ImageImportDescriptor {
    /// RVA to Import Lookup Table (ILT) / OriginalFirstThunk.
    /// 0 if bound imports (use FirstThunk).
    pub original_first_thunk: u32,
    pub time_date_stamp: u32,
    pub forwarder_chain: u32,
    /// RVA to null-terminated DLL name string.
    pub name: u32,
    /// RVA to Import Address Table (IAT).
    pub first_thunk: u32,
}

impl ImageImportDescriptor {
    pub const SIZE: usize = 20;

    /// Returns true if this is the null terminator entry.
    pub fn is_null(&self) -> bool {
        self.original_first_thunk == 0
            && self.name == 0
            && self.first_thunk == 0
            && self.time_date_stamp == 0
            && self.forwarder_chain == 0
    }
}

/// Import thunk for PE32+ (8 bytes each).
///
/// If the high bit (bit 63) is set: ordinal import (bits 0..15 = ordinal).
/// Otherwise: RVA to IMAGE_IMPORT_BY_NAME structure.
#[repr(C)]
pub union ImageThunkData64 {
    pub ordinal_and_flag: u64,
    pub address_of_data: u64,
    pub function: u64,
}

impl ImageThunkData64 {
    /// Ordinal flag bit (bit 63).
    pub const ORDINAL_FLAG: u64 = 1 << 63;

    pub fn is_ordinal(self) -> bool {
        unsafe { self.ordinal_and_flag & Self::ORDINAL_FLAG != 0 }
    }

    pub fn get_ordinal(self) -> u16 {
        unsafe { (self.ordinal_and_flag & 0xFFFF) as u16 }
    }

    pub fn get_hint_name_rva(self) -> u32 {
        unsafe { self.address_of_data as u32 }
    }
}

/// Hint/Name table entry for named imports.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ImageImportByName {
    /// Hint (index into export name pointer table).
    pub hint: u16,
    /// Function name (null-terminated, variable length).
    pub name: [u8; 1], // Flexible array member
}

/// Export directory (40 bytes).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ImageExportDirectory {
    pub characteristics: u32,
    pub time_date_stamp: u32,
    pub major_version: u16,
    pub minor_version: u16,
    /// RVA to DLL name string.
    pub name: u32,
    /// Ordinal base for exported functions.
    pub base: u32,
    /// Total number of exported functions (including ordinals).
    pub number_of_functions: u32,
    /// Number of named exports.
    pub number_of_names: u32,
    /// RVA to array of function addresses (RVA each).
    pub address_of_functions: u32,
    /// RVA to array of function name pointers (RVA each).
    pub address_of_names: u32,
    /// RVA to array of ordinal indices (u16 each).
    pub address_of_name_ordinals: u32,
}

impl ImageExportDirectory {
    pub const SIZE: usize = 40;
}

/// Base relocation block header (8 bytes).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ImageBaseRelocation {
    /// Page RVA — the base address of the page this block covers.
    pub virtual_address: u32,
    /// Block size in bytes, including this header.
    pub size_of_block: u32,
}

impl ImageBaseRelocation {
    pub const SIZE: usize = 8;

    /// Returns the number of relocation entries in this block.
    pub fn entry_count(&self) -> usize {
        if self.size_of_block <= Self::SIZE as u32 {
            return 0;
        }
        ((self.size_of_block - Self::SIZE as u32) / 2) as usize
    }
}

/// TLS directory for PE32+ (24 bytes).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ImageTlsDirectory64 {
    /// Start address of TLS raw data (template).
    pub start_address_of_raw_data: u64,
    /// End address of TLS raw data (template).
    pub end_address_of_raw_data: u64,
    /// Address of TLS index variable.
    pub address_of_index: u64,
    /// RVA to array of TLS callback function pointers.
    pub address_of_call_backs: u64,
    /// Size of zero-fill area after raw data.
    pub size_of_zero_fill: u32,
    pub characteristics: u32,
}

impl ImageTlsDirectory64 {
    pub const SIZE: usize = 24;

    /// Returns true if TLS is present.
    pub fn is_present(&self) -> bool {
        self.start_address_of_raw_data != 0 || self.address_of_index != 0
    }
}

/// Read a u16 from a byte slice at the given offset (little-endian).
///
/// # Panics
///
/// Panics if the offset + 2 exceeds the slice length.
#[inline]
pub fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// Read a u32 from a byte slice at the given offset (little-endian).
///
/// # Panics
///
/// Panics if the offset + 4 exceeds the slice length.
#[inline]
pub fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Read a u64 from a byte slice at the given offset (little-endian).
///
/// # Panics
///
/// Panics if the offset + 8 exceeds the slice length.
#[inline]
pub fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

/// Align `value` up to the next multiple of `align`.
#[inline]
pub const fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

/// Align `value` up to the next multiple of `align` (usize variant).
#[inline]
pub const fn align_up_usize(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// SVC instruction encoding on ARM64.
///
/// Format: `SVC #<imm16>`
/// Encoding: `0xD4000001 | (imm16 << 5)`
///
/// The immediate is bits [20:5] of the 32-bit instruction.
pub mod arm64_insn {
    /// SVC instruction base pattern: `0xD4000001`
    /// Full mask for matching any SVC: bits [24:21] = 0b0101, bits [20:5] = imm16, bits [4:0] = 0x01
    pub const SVC_MASK: u32 = 0xFFE0_001F;
    pub const SVC_PATTERN: u32 = 0xD400_0001;

    /// RET instruction: `0xD65F03C0`
    pub const RET: u32 = 0xD65F_03C0;

    /// MOV Xd, #imm16 encoding: `0xD2800000 | (Rd << 0) | (imm16 << 5)`
    /// For X8: `0xD2800000 | (8 << 0) | (imm16 << 5)` = `0xD2800008 | (imm16 << 5)`
    pub const MOV_X8_MASK: u32 = 0xFFE0_001F;
    pub const MOV_X8_PATTERN: u32 = 0xD280_0008;

    /// Extract the 16-bit immediate from an SVC instruction.
    #[inline]
    pub fn extract_svc_imm(instruction: u32) -> u16 {
        ((instruction >> 5) & 0xFFFF) as u16
    }

    /// Check if a 32-bit instruction is an SVC.
    #[inline]
    pub fn is_svc(instruction: u32) -> bool {
        (instruction & SVC_MASK) == SVC_PATTERN
    }

    /// Check if a 32-bit instruction is RET.
    #[inline]
    pub fn is_ret(instruction: u32) -> bool {
        instruction == RET
    }

    /// Check if a 32-bit instruction is `MOV X8, #imm16`.
    #[inline]
    pub fn is_mov_x8(instruction: u32) -> bool {
        (instruction & MOV_X8_MASK) == MOV_X8_PATTERN
    }

    /// Extract the 16-bit immediate from a MOV X8 instruction.
    #[inline]
    pub fn extract_mov_x8_imm(instruction: u32) -> u16 {
        ((instruction >> 5) & 0xFFFF) as u16
    }
}

/// Errors that can occur during PE loading.
#[derive(Debug, Clone, Copy)]
pub enum PeError {
    /// File is too small to contain a valid PE header.
    FileTooSmall,
    /// Invalid DOS magic (not "MZ").
    InvalidDosMagic,
    /// Invalid PE signature (not "PE\0\0").
    InvalidPeSignature,
    /// Unsupported machine type.
    UnsupportedMachineType(u16),
    /// Unsupported optional header magic (not PE32+).
    UnsupportedOptionalMagic(u16),
    /// Invalid section alignment or file alignment.
    InvalidAlignment,
    /// RVA outside of image bounds.
    InvalidRva(u32),
    /// Required data directory is missing.
    MissingDataDirectory(usize),
    /// Import resolution failed.
    ImportResolutionFailed,
    /// Relocation processing failed.
    RelocationFailed,
    /// Invalid relocation type.
    InvalidRelocationType(u16),
    /// TLS initialization failed.
    TlsInitFailed,
    /// Invalid export (ordinal or name not found).
    ExportNotFound,
}

impl PeError {
    /// Returns a static string description of the error.
    pub fn as_str(&self) -> &'static str {
        match self {
            PeError::FileTooSmall => "PE file too small",
            PeError::InvalidDosMagic => "Invalid DOS magic (expected MZ)",
            PeError::InvalidPeSignature => "Invalid PE signature (expected PE\\0\\0)",
            PeError::UnsupportedMachineType(m) => {
                // We can't format in const, so just return generic
                let _ = m;
                "Unsupported PE machine type"
            }
            PeError::UnsupportedOptionalMagic(m) => {
                let _ = m;
                "Unsupported optional header magic (expected PE32+)"
            }
            PeError::InvalidAlignment => "Invalid section/file alignment",
            PeError::InvalidRva(_) => "RVA out of image bounds",
            PeError::MissingDataDirectory(_) => "Required data directory missing",
            PeError::ImportResolutionFailed => "Failed to resolve imports",
            PeError::RelocationFailed => "Failed to process relocations",
            PeError::InvalidRelocationType(_) => "Unknown relocation type",
            PeError::TlsInitFailed => "TLS initialization failed",
            PeError::ExportNotFound => "Export not found",
        }
    }
}
