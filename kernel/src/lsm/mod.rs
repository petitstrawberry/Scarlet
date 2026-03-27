//! Loadable Scarlet Module (LSM) subsystem.
//!
//! Provides the infrastructure for loading relocatable ELF object files (`.o`)
//! into the kernel at runtime, resolving symbols against the kernel's exported
//! symbol table, and executing module initialisation functions.

pub mod elf;
pub mod loader;
pub mod symbol;
pub mod syscall;

pub use loader::{LsmError, ModuleHandle, load_module};

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
