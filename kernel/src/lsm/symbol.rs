use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::early_println;

unsafe extern "C" {
    static __SCARLET_KSYMS_START: u8;
    static __SCARLET_KSYMS_END: u8;
}

struct RegistryEntry {
    name: String,
    addr: usize,
    module_id: Option<u64>,
}

pub struct SymbolRegistry {
    entries: Vec<RegistryEntry>,
}

impl SymbolRegistry {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn lookup(&self, name: &str) -> Option<usize> {
        self.lookup_module(name).map(|(addr, _)| addr)
    }

    pub fn lookup_module(&self, name: &str) -> Option<(usize, Option<u64>)> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.name == name)
            .map(|entry| (entry.addr, entry.module_id))
    }

    pub fn register_module_symbols(&mut self, module_id: u64, symbols: &[(String, usize)]) {
        for (name, addr) in symbols {
            self.entries.push(RegistryEntry {
                name: name.clone(),
                addr: *addr,
                module_id: Some(module_id),
            });
        }
    }

    pub fn unregister_module_symbols(&mut self, module_id: u64) {
        self.entries
            .retain(|entry| entry.module_id != Some(module_id));
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

/// Parse the `.scarlet_ksyms` section (post-link binary blob).
///
/// Binary format (all little-endian u64):
/// ```text
/// u64  entry_count
/// [entry_count times]:
///   u64  addr
///   u64  name_len   (byte length, NOT including null terminator)
///   [name_len bytes] name (UTF-8, NOT null-terminated)
/// ```
pub fn init_kernel_symbols() {
    // SAFETY: __SCARLET_KSYMS_START/END are linker-defined symbols bounding
    // the .scarlet_ksyms section. The section is in read-only memory and was
    // populated by the post-link tool. If no post-link step ran, START == END
    // (or both are zero from the PROVIDE defaults) and the loop body is skipped.
    let start = unsafe { core::ptr::addr_of!(__SCARLET_KSYMS_START) as usize };
    let end = unsafe { core::ptr::addr_of!(__SCARLET_KSYMS_END) as usize };

    if start == 0 || end == 0 || start >= end {
        early_println!("[lsm] symbol table: no ksym section found, skipping");
        return;
    }

    let data = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    let mut registry = SYMBOL_REGISTRY.lock();

    let mut offset = 0usize;

    if data.len() < 8 {
        early_println!("[lsm] symbol table: ksym section too small, skipping");
        return;
    }

    let count = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0u8; 8])) as usize;
    offset = 8;

    let mut loaded = 0usize;
    let mut errors = 0usize;

    for _ in 0..count {
        if offset + 16 > data.len() {
            errors += 1;
            break;
        }
        let addr = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap_or([0; 8]));
        let name_len =
            u64::from_le_bytes(data[offset + 8..offset + 16].try_into().unwrap_or([0; 8]));
        offset += 16;

        let name_len = name_len as usize;
        if offset + name_len > data.len() {
            errors += 1;
            break;
        }

        let name_bytes = &data[offset..offset + name_len];
        offset += name_len;

        match core::str::from_utf8(name_bytes) {
            Ok(name) => {
                registry.entries.push(RegistryEntry {
                    name: String::from(name),
                    addr: addr as usize,
                    module_id: None,
                });
                loaded += 1;
            }
            Err(_) => {
                errors += 1;
            }
        }
    }

    early_println!(
        "[lsm] symbol table: {} symbol(s) registered ({} errors)",
        loaded,
        errors
    );
}

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

#[repr(C)]
pub struct KernelSymbol {
    pub name: *const u8,
    pub addr: usize,
}
