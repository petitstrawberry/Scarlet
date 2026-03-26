//! Loadable Scarlet Module (LSM) subsystem.
//!
//! Provides the infrastructure for loading relocatable ELF object files (`.o`)
//! into the kernel at runtime, resolving symbols against the kernel's exported
//! symbol table, and executing module initialisation functions.

pub mod elf;
pub mod arch;
pub mod symbol;
