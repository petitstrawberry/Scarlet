//! Loadable Scarlet Module (LSM) subsystem.
//!
//! Provides the infrastructure for loading relocatable ELF object files (`.o`)
//! into the kernel at runtime, resolving symbols against the kernel's exported
//! symbol table, and executing module initialisation functions.

pub mod arch;
pub mod elf;
pub mod loader;
pub mod symbol;

pub use loader::{load_module, LsmError, ModuleHandle};
