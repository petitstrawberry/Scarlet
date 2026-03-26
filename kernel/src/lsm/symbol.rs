//! Kernel Symbol Registry for Loadable Scarlet Modules (LSM).
//!
//! Provides the [`export_symbol!`] macro that places symbol metadata into the
//! `.scarlet_ksyms` linker section. At boot, [`init_kernel_symbols()`] walks
//! that section and populates a runtime [`SymbolRegistry`] that LSM loaders
//! query to resolve external references in relocatable object files.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::early_println;

/// A single exported kernel symbol: name + virtual address.
///
/// Instances are placed into `.scarlet_ksyms` by [`export_symbol!`].
#[repr(C)]
pub struct KernelSymbol {
    pub name: *const u8,
    pub addr: usize,
}

/// Runtime symbol registry populated at boot from `.scarlet_ksyms`.
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
            .map(|(_, &addr)| addr)
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
    static __SCARLET_KSYMS_START: KernelSymbol;
    static __SCARLET_KSYMS_END: KernelSymbol;
}

/// Walk the `.scarlet_ksyms` section and populate the global [`SymbolRegistry`].
///
/// Must be called once during kernel boot, after the kernel image is mapped.
pub fn init_kernel_symbols() {
    unsafe {
        let start = &__SCARLET_KSYMS_START as *const KernelSymbol;
        let end = &__SCARLET_KSYMS_END as *const KernelSymbol;

        if start >= end {
            early_println!("[lsm] symbol table: no exported symbols");
            return;
        }

        let mut count = 0usize;
        let mut ptr = start;

        while ptr < end {
            let sym = &*ptr;

            let name = if sym.name.is_null() {
                String::from("<null>")
            } else {
                let mut len = 0usize;
                while *sym.name.add(len) != 0 {
                    len += 1;
                }
                let bytes = core::slice::from_raw_parts(sym.name, len);
                String::from_utf8_lossy(bytes).into_owned()
            };

            SYMBOL_REGISTRY.lock().entries.push((name, sym.addr));
            count += 1;
            ptr = ptr.add(1);
        }

        early_println!("[lsm] symbol table: {} symbol(s) registered", count);
    }
}

/// Export a kernel symbol so that Loadable Scarlet Modules can resolve it at load time.
///
/// # Examples
///
/// ```ignore
/// #[scarlet::export_symbol("my_kernel_function")]
/// pub fn my_kernel_function() { /* ... */ }
/// ```
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
