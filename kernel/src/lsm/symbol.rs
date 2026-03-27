//! Kernel Symbol Registry for Loadable Scarlet Modules (LSM).
//!
//! At boot, [`init_kernel_symbols()`] parses the kernel's own `.symtab` section
//! (kept in memory by the linker script) and populates a runtime [`SymbolRegistry`]
//! with all globally-visible symbols. Optionally, symbols annotated with
//! [`export_symbol!`] are also registered.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::early_println;
use crate::lsm::elf::{self, SHT_SYMTAB, STB_GLOBAL, STB_WEAK};

/// Runtime symbol registry populated at boot.
pub struct SymbolRegistry {
    entries: Vec<(String, usize)>,
}

impl SymbolRegistry {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn lookup(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, addr)| *addr)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

static SYMBOL_REGISTRY: Mutex<SymbolRegistry> = Mutex::new(SymbolRegistry::new());

pub fn get_symbol_registry() -> &'static Mutex<SymbolRegistry> {
    &SYMBOL_REGISTRY
}

unsafe extern "C" {
    static __SYMTAB_START: u8;
    static __SYMTAB_END: u8;
    static __STRTAB_START: u8;
    static __STRTAB_END: u8;
    static __SCARLET_KSYMS_START: u8;
    static __SCARLET_KSYMS_END: u8;
}

/// Walk the kernel's `.symtab` and register all STB_GLOBAL/STB_WEAK defined symbols.
///
/// Also walks `.scarlet_ksyms` for any explicitly annotated symbols.
/// Must be called once during kernel boot, after the kernel image is mapped.
pub fn init_kernel_symbols() {
    unsafe {
        let symtab_start = &__SYMTAB_START as *const u8 as usize;
        let symtab_end = &__SYMTAB_END as *const u8 as usize;
        let strtab_start = &__STRTAB_START as *const u8 as usize;
        let strtab_end = &__STRTAB_END as *const u8 as usize;

        if symtab_start < symtab_end && strtab_start < strtab_end {
            let symtab_data =
                core::slice::from_raw_parts(symtab_start as *const u8, symtab_end - symtab_start);
            let strtab_data =
                core::slice::from_raw_parts(strtab_start as *const u8, strtab_end - strtab_start);

            let sym_count = (symtab_end - symtab_start) / elf::ELF64_SYM_SIZE;
            let mut count = 0usize;

            for i in 0..sym_count {
                let off = i * elf::ELF64_SYM_SIZE;
                if off + elf::ELF64_SYM_SIZE > symtab_data.len() {
                    break;
                }

                let sym = match elf::parse_symtab_entry(symtab_data, off, true) {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let bind = sym.st_info >> 4;
                let shndx = sym.st_shndx;

                if shndx == 0 {
                    continue;
                }

                if bind != STB_GLOBAL && bind != STB_WEAK {
                    continue;
                }

                if sym.st_value == 0 {
                    continue;
                }

                let name = if sym.st_name as usize >= strtab_data.len() {
                    continue;
                } else {
                    let start = sym.st_name as usize;
                    let end = strtab_data[start..]
                        .iter()
                        .position(|&b| b == 0)
                        .map(|p| start + p)
                        .unwrap_or(strtab_data.len());
                    let bytes = &strtab_data[start..end];
                    String::from_utf8_lossy(bytes).into_owned()
                };

                SYMBOL_REGISTRY
                    .lock()
                    .entries
                    .push((name, sym.st_value as usize));
                count += 1;
            }

            early_println!("[lsm] symbol table: {} symbol(s) from .symtab", count);
        } else {
            early_println!("[lsm] symbol table: .symtab not found in memory");
        }

        let ksyms_start = &__SCARLET_KSYMS_START as *const u8 as usize;
        let ksyms_end = &__SCARLET_KSYMS_END as *const u8 as usize;

        if ksyms_start < ksyms_end {
            let ksyms_data =
                core::slice::from_raw_parts(ksyms_start as *const u8, ksyms_end - ksyms_start);
            let ksym_stride = core::mem::size_of::<KernelSymbol>();
            let mut explicit_count = 0usize;
            let mut ptr = ksyms_start;

            while ptr + ksym_stride <= ksyms_end {
                let ksym = &*(ptr as *const KernelSymbol);

                if !ksym.name.is_null() {
                    let mut len = 0usize;
                    while *ksym.name.add(len) != 0 {
                        len += 1;
                    }
                    let bytes = core::slice::from_raw_parts(ksym.name, len);
                    let name = String::from_utf8_lossy(bytes).into_owned();

                    SYMBOL_REGISTRY.lock().entries.push((name, ksym.addr));
                    explicit_count += 1;
                }

                ptr += ksym_stride;
            }

            if explicit_count > 0 {
                early_println!("[lsm] symbol table: {} explicit export(s)", explicit_count);
            }
        }
    }
}

/// A single explicitly exported kernel symbol (placed by [`export_symbol!`]).
#[repr(C)]
pub struct KernelSymbol {
    pub name: *const u8,
    pub addr: usize,
}

/// Export a kernel symbol explicitly (marks it as stable module API).
///
/// Symbols annotated with this macro are guaranteed to be available for
/// module resolution even if the automatic `.symtab` extraction misses them.
#[macro_export]
macro_rules! export_symbol {
    ($name:literal) => {
        $crate::lsm::symbol::_export_symbol_impl!($name, $name);
    };
    ($name:literal, $path:path) => {
        $crate::lsm::symbol::_export_symbol_impl!($name, $path);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! _export_symbol_impl {
    ($name:literal, $addr:expr) => {
        #[unsafe(link_section = ".scarlet_ksyms")]
        #[used]
        static __SCARLET_KSYM: $crate::lsm::symbol::KernelSymbol =
            $crate::lsm::symbol::KernelSymbol {
                name: concat!($name, "\0").as_ptr(),
                addr: $addr as usize,
            };
    };
}
