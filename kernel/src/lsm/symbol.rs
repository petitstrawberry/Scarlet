#![allow(clippy::all)]
#![allow(dead_code)]

include!("generated_symbols.rs");

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::early_println;

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

pub fn init_kernel_symbols() {
    let syms = get_kernel_symbols();
    let mut registry = SYMBOL_REGISTRY.lock();

    for &(name, addr) in syms {
        registry.entries.push((String::from(name), addr));
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
