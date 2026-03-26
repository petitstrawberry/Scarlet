use alloc::string::String;
use alloc::vec::Vec;

use core::mem;
use spin::Mutex;

use crate::environment::PAGE_SIZE;
use crate::lsm::arch;
use crate::lsm::elf::{
    self, SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHN_UNDEF, SHT_NOBITS, SHT_PROGBITS,
};
use crate::lsm::symbol;
use crate::mem::page::allocate_raw_pages;
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
}

#[derive(Debug)]
pub struct ModuleHandle {
    pub name: String,
    section_bases: Vec<(usize, usize)>,
    mapped_ranges: Vec<(usize, usize)>,
    initialized: bool,
}

#[cfg(target_arch = "riscv64")]
const MODULE_VA_START: usize = 0xffffffff90000000;
#[cfg(target_arch = "riscv64")]
const MODULE_VA_SIZE: usize = 256 * 1024 * 1024;

#[cfg(target_arch = "riscv64")]
static MODULE_VA_OFFSET: Mutex<usize> = Mutex::new(0);

#[cfg(target_arch = "riscv64")]
fn allocate_module_pages(size: usize, alignment: usize) -> Option<usize> {
    let mut offset = MODULE_VA_OFFSET.lock();
    let aligned = (*offset + alignment - 1) & !(alignment - 1);
    if aligned + size > MODULE_VA_SIZE {
        return None;
    }
    *offset = aligned + size;
    Some(MODULE_VA_START + aligned)
}

#[cfg(target_arch = "riscv64")]
fn section_alignment(align: u64) -> usize {
    let raw = usize::try_from(align).ok().unwrap_or(PAGE_SIZE);
    if raw.is_power_of_two() && raw > PAGE_SIZE {
        raw
    } else {
        PAGE_SIZE
    }
}

#[cfg(target_arch = "riscv64")]
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

#[cfg(target_arch = "riscv64")]
fn round_up_to_page(size: usize) -> usize {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

#[cfg(target_arch = "riscv64")]
fn flush_icache_all() {
    unsafe {
        core::arch::asm!("fence.i", options(nostack));
    }
}

#[cfg(target_arch = "riscv64")]
fn section_base_for(section_bases: &[(usize, usize)], section_index: usize) -> Option<usize> {
    section_bases
        .iter()
        .find_map(|(idx, base)| (*idx == section_index).then_some(*base))
}

#[cfg(target_arch = "riscv64")]
fn infer_module_name(object: &elf::RelocObject) -> String {
    object
        .symbols
        .iter()
        .find(|symbol| symbol.typ == elf::STT_FILE && !symbol.name.is_empty())
        .map(|symbol| symbol.name.clone())
        .unwrap_or_else(|| String::from("lsm-module"))
}

#[cfg(target_arch = "riscv64")]
pub fn load_module(data: &[u8]) -> Result<ModuleHandle, LsmError> {
    let object = elf::parse_reloc_object(data).map_err(LsmError::InvalidElf)?;

    let kernel_vm = get_kernel_vm_manager();
    let kernel_asid = kernel_vm.get_asid();

    let mut section_bases: Vec<(usize, usize)> = Vec::new();
    let mut mapped_ranges: Vec<(usize, usize)> = Vec::new();

    for (section_index, section) in object.sections.iter().enumerate() {
        if (section.sh_flags & SHF_ALLOC) == 0 {
            continue;
        }

        let section_size = usize::try_from(section.sh_size).map_err(|_| LsmError::NoMemory)?;
        if section_size == 0 {
            continue;
        }

        let mapped_size = round_up_to_page(section_size);
        let alignment = section_alignment(section.sh_addralign);
        let base_vaddr = allocate_module_pages(mapped_size, alignment).ok_or(LsmError::NoMemory)?;
        let num_pages = mapped_size / PAGE_SIZE;

        let pages_ptr = allocate_raw_pages(num_pages);
        if pages_ptr.is_null() {
            return Err(LsmError::NoMemory);
        }

        let base_paddr = virt_to_phys(pages_ptr as usize);
        let permissions = section_permissions(section.sh_flags);

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
            .map_err(|_| LsmError::NoMemory)?;
        if !overwritten.is_empty() {
            return Err(LsmError::NoMemory);
        }

        let root_page_table = kernel_vm.get_root_page_table().ok_or(LsmError::NoMemory)?;
        root_page_table
            .map_memory_area(kernel_asid, memory_map, true, true)
            .map_err(|_| LsmError::NoMemory)?;

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
    }

    let symbol_resolver = |name: &str| symbol::get_symbol_registry().lock().lookup(name);

    arch::apply_relocations(&object, &section_bases, &symbol_resolver)
        .map_err(LsmError::Relocation)?;

    flush_icache_all();

    let init_symbol = object
        .symbols
        .iter()
        .find(|sym| sym.name == "scarlet_lsm_init")
        .ok_or(LsmError::NoInitSymbol)?;

    if init_symbol.shndx == SHN_UNDEF {
        return Err(LsmError::NoInitSymbol);
    }

    let init_section_index = usize::from(init_symbol.shndx);
    let init_base =
        section_base_for(&section_bases, init_section_index).ok_or(LsmError::NoInitSymbol)?;
    let init_offset = usize::try_from(init_symbol.value).map_err(|_| LsmError::NoInitSymbol)?;
    let init_addr = init_base
        .checked_add(init_offset)
        .ok_or(LsmError::NoInitSymbol)?;

    let init_fn: fn() -> Result<(), &'static str> = unsafe { mem::transmute(init_addr) };
    init_fn().map_err(LsmError::InitFailed)?;

    Ok(ModuleHandle {
        name: infer_module_name(&object),
        section_bases,
        mapped_ranges,
        initialized: true,
    })
}

#[cfg(not(target_arch = "riscv64"))]
pub fn load_module(_data: &[u8]) -> Result<ModuleHandle, LsmError> {
    Err(LsmError::InvalidElf(
        "Loadable Scarlet Module loader currently supports riscv64 only",
    ))
}
