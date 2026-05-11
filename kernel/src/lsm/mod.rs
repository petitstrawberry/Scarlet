//! Loadable Scarlet Module (LSM) subsystem.
//!
//! Provides the infrastructure for loading relocatable ELF object files (`.o`)
//! into the kernel at runtime, resolving symbols against the kernel's exported
//! symbol table, and executing module initialisation functions.

pub mod elf;
pub mod loader;
pub mod symbol;
pub mod syscall;

pub use loader::{LoadedModule, LsmError, list_modules, load_module, unload_module};

#[derive(Debug)]
pub enum RelocateError {
    UnresolvedSymbol(alloc::string::String),
    Relocation(&'static str),
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LsmErrorCode {
    Success = 0,
    InvalidPath = 1,
    InvalidElf = 2,
    NoMemory = 3,
    RelocationError = 4,
    NoInit = 5,
    InitFailed = 6,
    BuildInfoMismatch = 7,
    NotFound = 8,
    PermissionDenied = 9,
    MissingDependency = 10,
    ArchMismatch = 11,
    UnresolvedSymbol = 12,
}

#[unsafe(no_mangle)]
pub extern "C" fn lsm_print(s: *const u8, len: usize) {
    if s.is_null() || len == 0 {
        return;
    }
    let slice = unsafe { core::slice::from_raw_parts(s, len) };
    if let Ok(text) = core::str::from_utf8(slice) {
        crate::early_print!("{}", text);
    }
}
