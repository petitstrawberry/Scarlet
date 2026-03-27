use alloc::string::String;
use alloc::vec::Vec;

use core::mem;
use spin::Mutex;

use crate::environment::PAGE_SIZE;
use crate::lsm::elf::{
    self, SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHN_UNDEF, SHT_NOBITS, SHT_PROGBITS,
};
use crate::lsm::symbol;
use crate::mem::page::{Page, allocate_raw_pages, free_raw_pages};
use crate::vm::addr::virt_to_phys;
use crate::vm::get_kernel_vm_manager;
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};

#[derive(Debug)]
pub enum LsmError {
    InvalidElf(&'static str),
    NoMemory,
    Relocation(&'static str),
    NoInitSymbol,
    InitFailed(&'static str),
    BuildInfoMismatch,
    NotFound,
    PermissionDenied,
}

#[derive(Debug)]
pub struct LoadedModule {
    pub id: u64,
    pub name: String,
    pub section_bases: Vec<(usize, usize)>,
    pub mapped_ranges: Vec<(usize, usize)>,
    pub pages_ptrs: Vec<*mut Page>,
    pub initialized: bool,
}

unsafe impl Send for LoadedModule {}
unsafe impl Sync for LoadedModule {}

use crate::arch::lsm::MODULE_VA_START;

const MODULE_VA_SIZE: usize = 256 * 1024 * 1024;

static MODULE_VA_OFFSET: Mutex<usize> = Mutex::new(0);
static MODULE_REGISTRY: Mutex<Vec<LoadedModule>> = Mutex::new(Vec::new());
static NEXT_MODULE_ID: Mutex<u64> = Mutex::new(1);

fn allocate_module_pages(size: usize, alignment: usize) -> Option<usize> {
    let mut offset = MODULE_VA_OFFSET.lock();
    let aligned = (*offset + alignment - 1) & !(alignment - 1);
    if aligned + size > MODULE_VA_SIZE {
        return None;
    }
    *offset = aligned + size;
    Some(MODULE_VA_START + aligned)
}

fn section_alignment(align: u64) -> usize {
    let raw = usize::try_from(align).ok().unwrap_or(PAGE_SIZE);
    if raw.is_power_of_two() && raw > PAGE_SIZE {
        raw
    } else {
        PAGE_SIZE
    }
}

fn section_permissions(flags: u64) -> usize {
    let mut permissions = VirtualMemoryPermission::Read as usize;
    if (flags & SHF_WRITE) != 0 {
        permissions |= VirtualMemoryPermission::Write as usize;
    }
    if (flags & SHF_EXECINSTR) != 0 {
        permissions |= VirtualMemoryPermission::Execute as usize;
    }
    permissions
}

fn loading_permissions(flags: u64) -> usize {
    section_permissions(flags) | VirtualMemoryPermission::Write as usize
}

fn final_permissions(flags: u64) -> usize {
    let mut permissions = section_permissions(flags);
    if (flags & SHF_EXECINSTR) != 0 {
        permissions &= !(VirtualMemoryPermission::Write as usize);
    }
    permissions
}

fn round_up_to_page(size: usize) -> usize {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

#[cfg(target_arch = "riscv64")]
fn flush_icache_all(_mapped_ranges: &[(usize, usize)]) {
    unsafe {
        core::arch::asm!("fence.i", options(nostack));
    }
}

#[cfg(target_arch = "aarch64")]
fn flush_icache_all(mapped_ranges: &[(usize, usize)]) {
    for &(start, size) in mapped_ranges {
        crate::arch::aarch64::clean_dcache_to_pou_range(start, size);
    }
    unsafe {
        core::arch::asm!("ic iallu", "dsb ish", "isb", options(nostack));
    }
}

fn section_base_for(section_bases: &[(usize, usize)], section_index: usize) -> Option<usize> {
    section_bases
        .iter()
        .find_map(|(idx, base)| (*idx == section_index).then_some(*base))
}

const KERNEL_BUILD_INFO: &str = concat!(env!("RUSTC_VERSION"), ";", env!("TARGET"));

fn read_module_string(
    object: &elf::RelocObject,
    section_bases: &[(usize, usize)],
    symbol_name: &str,
    max_len: usize,
) -> Option<String> {
    let sym = object.symbols.iter().find(|s| s.name == symbol_name)?;
    let shndx = sym.shndx as usize;
    let base = section_bases.iter().find(|(idx, _)| *idx == shndx)?.1;
    let offset = sym.value as usize;
    let ptr = (base + offset) as *const u8;
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 && len < max_len {
            len += 1;
        }
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(bytes).ok().map(String::from)
}

fn resolve_module_name(object: &elf::RelocObject, section_bases: &[(usize, usize)]) -> String {
    read_module_string(object, section_bases, "SCARLET_LSM_NAME", 256)
        .unwrap_or_else(|| String::from("lsm-module"))
}

fn rollback_mappings_and_pages(
    mapped_ranges: &[(usize, usize)],
    pages_ptrs: &[*mut Page],
    pages_counts: &[usize],
) {
    let kernel_vm = get_kernel_vm_manager();
    for &(vaddr, _) in mapped_ranges {
        let _ = kernel_vm.remove_memory_map_by_addr(vaddr);
    }
    for (&ptr, &num_pages) in pages_ptrs.iter().zip(pages_counts.iter()) {
        free_raw_pages(ptr, num_pages);
    }
}

pub fn unload_module(module_id: u64) -> Result<(), LsmError> {
    let module = {
        let mut registry = MODULE_REGISTRY.lock();
        let pos = registry
            .iter()
            .position(|module| module.id == module_id)
            .ok_or(LsmError::NotFound)?;
        registry.remove(pos)
    };

    let kernel_vm = get_kernel_vm_manager();
    for &(vaddr, _) in &module.mapped_ranges {
        let _ = kernel_vm.remove_memory_map_by_addr(vaddr);
    }
    for (&ptr, &(_, size)) in module.pages_ptrs.iter().zip(module.mapped_ranges.iter()) {
        free_raw_pages(ptr, size / PAGE_SIZE);
    }

    Ok(())
}

pub fn list_modules() -> Vec<(u64, String)> {
    MODULE_REGISTRY
        .lock()
        .iter()
        .map(|module| (module.id, module.name.clone()))
        .collect()
}

pub fn load_module(data: &[u8]) -> Result<u64, LsmError> {
    let object = elf::parse_reloc_object(data).map_err(LsmError::InvalidElf)?;

    let kernel_vm = get_kernel_vm_manager();
    let kernel_asid = kernel_vm.get_asid();

    let mut section_bases: Vec<(usize, usize)> = Vec::new();
    let mut mapped_ranges: Vec<(usize, usize)> = Vec::new();
    let mut pages_ptrs: Vec<*mut Page> = Vec::new();
    let mut pages_counts: Vec<usize> = Vec::new();
    let mut section_flags: Vec<(usize, u64)> = Vec::new();

    for (section_index, section) in object.sections.iter().enumerate() {
        if (section.sh_flags & SHF_ALLOC) == 0 {
            continue;
        }

        let section_size = match usize::try_from(section.sh_size) {
            Ok(size) => size,
            Err(_) => {
                rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
                return Err(LsmError::NoMemory);
            }
        };
        if section_size == 0 {
            continue;
        }

        let mapped_size = round_up_to_page(section_size);
        let alignment = section_alignment(section.sh_addralign);
        let base_vaddr = match allocate_module_pages(mapped_size, alignment) {
            Some(vaddr) => vaddr,
            None => {
                rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
                return Err(LsmError::NoMemory);
            }
        };
        let num_pages = mapped_size / PAGE_SIZE;

        let pages_ptr = allocate_raw_pages(num_pages);
        if pages_ptr.is_null() {
            rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
            return Err(LsmError::NoMemory);
        }

        let base_paddr = virt_to_phys(pages_ptr as usize);
        let permissions = loading_permissions(section.sh_flags);

        let memory_map = VirtualMemoryMap {
            pmarea: MemoryArea {
                start: base_paddr,
                end: base_paddr + mapped_size - 1,
            },
            vmarea: MemoryArea {
                start: base_vaddr,
                end: base_vaddr + mapped_size - 1,
            },
            permissions,
            is_shared: true,
            owner: None,
        };

        let overwritten = kernel_vm
            .add_memory_map_fixed(memory_map.clone())
            .map_err(|_| {
                free_raw_pages(pages_ptr, num_pages);
                rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
                LsmError::NoMemory
            })?;
        if !overwritten.is_empty() {
            let _ = kernel_vm.remove_memory_map_by_addr(base_vaddr);
            free_raw_pages(pages_ptr, num_pages);
            rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
            return Err(LsmError::NoMemory);
        }

        let root_page_table = match kernel_vm.get_root_page_table() {
            Some(pt) => pt,
            None => {
                let _ = kernel_vm.remove_memory_map_by_addr(base_vaddr);
                free_raw_pages(pages_ptr, num_pages);
                rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
                return Err(LsmError::NoMemory);
            }
        };
        root_page_table
            .map_memory_area(kernel_asid, memory_map, true, true)
            .map_err(|_| {
                let _ = kernel_vm.remove_memory_map_by_addr(base_vaddr);
                free_raw_pages(pages_ptr, num_pages);
                rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
                LsmError::NoMemory
            })?;

        let section_dst = base_vaddr as *mut u8;
        match section.sh_type {
            SHT_PROGBITS => {
                let section_data = &object.section_data[section.data_index];
                let copy_len = core::cmp::min(section_data.len(), section_size);
                unsafe {
                    core::ptr::copy_nonoverlapping(section_data.as_ptr(), section_dst, copy_len);
                }
            }
            SHT_NOBITS => unsafe {
                core::ptr::write_bytes(section_dst, 0, section_size);
            },
            _ => {}
        }

        section_bases.push((section_index, base_vaddr));
        mapped_ranges.push((base_vaddr, mapped_size));
        pages_ptrs.push(pages_ptr);
        pages_counts.push(num_pages);
        section_flags.push((section_index, section.sh_flags));
    }

    let module_build_info =
        read_module_string(&object, &section_bases, "SCARLET_LSM_BUILD_INFO", 256);
    if let Some(ref info) = module_build_info {
        if info != KERNEL_BUILD_INFO {
            rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
            return Err(LsmError::BuildInfoMismatch);
        }
    }

    let symbol_resolver = |name: &str| symbol::get_symbol_registry().lock().lookup(name);

    crate::arch::lsm::apply_relocations(&object, &section_bases, &symbol_resolver).map_err(
        |e| {
            rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
            LsmError::Relocation(e)
        },
    )?;

    flush_icache_all(&mapped_ranges);

    {
        let root_page_table = kernel_vm.get_root_page_table().ok_or_else(|| {
            rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
            LsmError::NoMemory
        })?;

        for (((section_index, base_vaddr), &(_, mapped_size)), &pages_ptr) in section_bases
            .iter()
            .zip(mapped_ranges.iter())
            .zip(pages_ptrs.iter())
        {
            let flags = section_flags
                .iter()
                .find_map(|(idx, flags)| (*idx == *section_index).then_some(*flags))
                .unwrap_or(0);
            let permissions = final_permissions(flags);
            let num_pages = mapped_size / PAGE_SIZE;
            for page_index in 0..num_pages {
                let page_vaddr = *base_vaddr + page_index * PAGE_SIZE;
                let page_paddr = virt_to_phys(pages_ptr as usize + page_index * PAGE_SIZE);
                root_page_table.map(
                    kernel_asid,
                    page_vaddr,
                    page_paddr,
                    permissions,
                    true,
                    VirtualMemoryPermission::Write.contained_in(permissions),
                );
            }
        }
    }

    let init_symbol = object
        .symbols
        .iter()
        .find(|sym| sym.name == "scarlet_lsm_init")
        .ok_or_else(|| {
            rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
            LsmError::NoInitSymbol
        })?;

    if init_symbol.shndx == SHN_UNDEF {
        rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
        return Err(LsmError::NoInitSymbol);
    }

    let init_section_index = usize::from(init_symbol.shndx);
    let init_base = section_base_for(&section_bases, init_section_index).ok_or_else(|| {
        rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
        LsmError::NoInitSymbol
    })?;
    let init_offset = usize::try_from(init_symbol.value).map_err(|_| {
        rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
        LsmError::NoInitSymbol
    })?;
    let init_addr = init_base.checked_add(init_offset).ok_or_else(|| {
        rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
        LsmError::NoInitSymbol
    })?;

    let init_fn: fn() -> Result<(), &'static str> = unsafe { mem::transmute(init_addr) };
    init_fn().map_err(|e| {
        rollback_mappings_and_pages(&mapped_ranges, &pages_ptrs, &pages_counts);
        LsmError::InitFailed(e)
    })?;

    let module_id = {
        let mut next_module_id = NEXT_MODULE_ID.lock();
        let id = *next_module_id;
        *next_module_id = next_module_id.saturating_add(1);
        id
    };

    MODULE_REGISTRY.lock().push(LoadedModule {
        id: module_id,
        name: resolve_module_name(&object, &section_bases),
        section_bases,
        mapped_ranges,
        pages_ptrs,
        initialized: true,
    });

    Ok(module_id)
}
