#![allow(clippy::all)]
#![allow(dead_code)]

include!("generated_symbols.rs");

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::early_println;

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
        let normalized = strip_crate_hash(name);
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.name == normalized)
            .map(|entry| (entry.addr, entry.module_id))
    }

    pub fn register_module_symbols(&mut self, module_id: u64, symbols: &[(String, usize)]) {
        for (name, addr) in symbols {
            let normalized = strip_crate_hash(name);
            self.entries.push(RegistryEntry {
                name: normalized.into_owned(),
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

fn strip_crate_hash(name: &str) -> alloc::borrow::Cow<'_, str> {
    if !name.contains("NtC") {
        return alloc::borrow::Cow::Borrowed(name);
    }
    let mut result = alloc::string::String::with_capacity(name.len());
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 3 < bytes.len()
            && bytes[i] == b'N'
            && bytes[i + 1] == b't'
            && bytes[i + 2] == b'C'
            && i + 15 <= bytes.len()
            && bytes[i + 3] != b'_'
        {
            let hash_end = i + 15;
            if bytes[i + 3..hash_end]
                .iter()
                .all(|b| b.is_ascii_alphanumeric())
                && (hash_end >= bytes.len() || bytes[hash_end] == b'_')
            {
                result.push_str("NtC");
                i = hash_end;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    alloc::borrow::Cow::Owned(result)
}

pub fn init_kernel_symbols() {
    let syms = get_kernel_symbols();
    let mut registry = SYMBOL_REGISTRY.lock();

    for &(name, addr) in syms {
        registry.entries.push(RegistryEntry {
            name: strip_crate_hash(name).into_owned(),
            addr,
            module_id: None,
        });
    }

    early_println!(
        "[lsm] symbol table: {} symbol(s) registered",
        registry.entries.len()
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
