use crate::arch::vm::mmu::{PageTable as ArchPageTable, PageTableEntry as ArchPageTableEntry};
use crate::environment::{KERNEL_HEAP_BASE, PAGE_SIZE, SCARLET_HHDM_BASE};
use crate::mem::pmm;
use crate::vm::addr::{boot_phys_to_virt, kernel_virt_to_phys};
use crate::vm::direct_map::DirectMapRegions;
use crate::vm::vmem::{MemoryArea, MemoryAttribute, VirtualMemoryPermission};

const BOOT_ASID: u16 = 0;

fn align_down(addr: usize, align: usize) -> usize {
    addr & !(align - 1)
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

fn direct_map_virtual_area(physical_area: MemoryArea) -> MemoryArea {
    MemoryArea {
        start: SCARLET_HHDM_BASE
            .checked_add(physical_area.start)
            .expect("direct-map virtual start overflows"),
        end: SCARLET_HHDM_BASE
            .checked_add(physical_area.end)
            .expect("direct-map virtual end overflows"),
    }
}

fn alloc_boot_pagetable() -> (usize, *mut ArchPageTable) {
    let paddr = pmm::alloc_frame().expect("Failed to allocate boot page table frame");
    let vaddr = boot_phys_to_virt(paddr);
    unsafe {
        core::ptr::write_bytes(vaddr as *mut u8, 0, PAGE_SIZE);
        #[cfg(target_arch = "aarch64")]
        crate::arch::aarch64::clean_dcache_to_poc_range(vaddr, PAGE_SIZE);
    }
    (paddr, vaddr as *mut ArchPageTable)
}

#[cfg(target_arch = "riscv64")]
fn boot_walk(
    root: *mut ArchPageTable,
    vaddr: usize,
    alloc: bool,
) -> Option<&'static mut ArchPageTableEntry> {
    let canonical_check = (vaddr >> 47) & 1;
    let upper_bits = (vaddr >> 48) & 0xffff;
    if canonical_check == 1 && upper_bits != 0xffff {
        return None;
    } else if canonical_check == 0 && upper_bits != 0 {
        return None;
    }

    let mut pagetable = root;
    unsafe {
        for level in (1..=3).rev() {
            let vpn = (vaddr >> (12 + 9 * level)) & 0x1ff;
            let pte = &mut (*pagetable).entries[vpn];

            if pte.is_valid() {
                if pte.is_leaf() {
                    return None;
                }
                pagetable = boot_phys_to_virt(pte.get_ppn() << 12) as *mut ArchPageTable;
            } else {
                if !alloc {
                    return None;
                }
                let (new_table_paddr, new_table) = alloc_boot_pagetable();
                pte.clear_all();
                pte.set_ppn(new_table_paddr >> 12);
                pte.validate();
                pagetable = new_table;
            }
        }

        let vpn = (vaddr >> 12) & 0x1ff;
        Some(&mut (*pagetable).entries[vpn])
    }
}

#[cfg(target_arch = "riscv64")]
fn boot_map_page(
    root: *mut ArchPageTable,
    vaddr: usize,
    paddr: usize,
    permissions: usize,
    _memory_attribute: MemoryAttribute,
) {
    let vaddr = vaddr & !(PAGE_SIZE - 1);
    let paddr = paddr & !(PAGE_SIZE - 1);
    let pte = boot_walk(root, vaddr, true).expect("boot_map_page: failed to allocate walk path");

    pte.clear_all();
    if VirtualMemoryPermission::Read.contained_in(permissions) {
        pte.readable();
    }
    if VirtualMemoryPermission::Write.contained_in(permissions) {
        pte.writable();
    }
    if VirtualMemoryPermission::Execute.contained_in(permissions) {
        pte.executable();
    }
    if VirtualMemoryPermission::User.contained_in(permissions) {
        pte.accesible_from_user();
    }
    pte.accessed();
    pte.dirty();
    pte.set_ppn(paddr >> 12);
    pte.validate();
}

#[cfg(target_arch = "aarch64")]
fn boot_walk(
    root: *mut ArchPageTable,
    vaddr: usize,
    alloc: bool,
) -> Option<&'static mut ArchPageTableEntry> {
    let upper = vaddr >> 48;
    if upper != 0 && upper != 0xffff {
        return None;
    }

    let mut pagetable = root;
    unsafe {
        for level in 0..3 {
            let shift = 12 + 9 * (3 - level);
            let index = (vaddr >> shift) & 0x1ff;
            let pte = &mut (*pagetable).entries[index];

            if pte.is_valid() {
                if !pte.is_table() {
                    return None;
                }
                pagetable = boot_phys_to_virt(pte.get_ppn() << 12) as *mut ArchPageTable;
            } else {
                if !alloc {
                    return None;
                }
                let (new_table_paddr, new_table) = alloc_boot_pagetable();
                pte.clear_all();
                pte.set_ppn(new_table_paddr >> 12);
                pte.set_table();
                crate::arch::aarch64::clean_dcache_to_poc_range(
                    (pte as *const ArchPageTableEntry) as usize,
                    core::mem::size_of::<ArchPageTableEntry>(),
                );
                pagetable = new_table;
            }
        }

        let index = (vaddr >> 12) & 0x1ff;
        Some(&mut (*pagetable).entries[index])
    }
}

#[cfg(target_arch = "aarch64")]
fn boot_map_page(
    root: *mut ArchPageTable,
    vaddr: usize,
    paddr: usize,
    permissions: usize,
    memory_attribute: MemoryAttribute,
) {
    let upper = vaddr >> 48;
    if upper != 0 && upper != 0xffff {
        panic!(
            "boot_map_page: non-canonical AArch64 virtual address {:#x}",
            vaddr
        );
    }

    let vaddr = vaddr & !(PAGE_SIZE - 1);
    let paddr = paddr & !(PAGE_SIZE - 1);
    let pte = boot_walk(root, vaddr, true).expect("boot_map_page: failed to allocate walk path");

    pte.set_entry(ArchPageTable::make_leaf_entry(
        vaddr,
        paddr,
        permissions,
        0,
        memory_attribute,
    ));
    crate::arch::aarch64::clean_dcache_to_poc_range(
        (pte as *const ArchPageTableEntry) as usize,
        core::mem::size_of::<ArchPageTableEntry>(),
    );
}

fn boot_map_range(
    root: *mut ArchPageTable,
    varea: MemoryArea,
    parea: MemoryArea,
    permissions: usize,
    memory_attribute: MemoryAttribute,
) {
    if varea.start % PAGE_SIZE != 0
        || parea.start % PAGE_SIZE != 0
        || varea.size() % PAGE_SIZE != 0
        || parea.size() % PAGE_SIZE != 0
    {
        panic!("boot_map_range: unaligned mapping request");
    }
    if varea.size() != parea.size() {
        panic!("boot_map_range: size mismatch between virtual and physical areas");
    }

    let mut vaddr = varea.start;
    let mut paddr = parea.start;
    while vaddr <= varea.end.saturating_sub(PAGE_SIZE - 1) {
        boot_map_page(root, vaddr, paddr, permissions, memory_attribute);
        vaddr = vaddr
            .checked_add(PAGE_SIZE)
            .expect("boot_map_range: vaddr overflow");
        paddr = paddr
            .checked_add(PAGE_SIZE)
            .expect("boot_map_range: paddr overflow");
    }
}

#[allow(static_mut_refs)]
pub fn switch_to_boot_page_table(
    direct_map_regions: DirectMapRegions,
    initramfs_paddr: Option<MemoryArea>,
    heap_paddr: MemoryArea,
) {
    unsafe extern "C" {
        static __KERNEL_SPACE_START: usize;
        static __KERNEL_SPACE_END: usize;
    }

    let (_, root) = alloc_boot_pagetable();

    let kernel_area = MemoryArea {
        start: align_down(
            unsafe { &__KERNEL_SPACE_START as *const usize as usize },
            PAGE_SIZE,
        ),
        end: align_up(
            unsafe { &__KERNEL_SPACE_END as *const usize as usize },
            PAGE_SIZE,
        ) - 1,
    };
    let kernel_phys_area = MemoryArea {
        start: align_down(kernel_virt_to_phys(kernel_area.start), PAGE_SIZE),
        end: align_up(kernel_virt_to_phys(kernel_area.end) + 1, PAGE_SIZE) - 1,
    };

    let heap_phys_area = MemoryArea {
        start: align_down(heap_paddr.start, PAGE_SIZE),
        end: align_up(heap_paddr.end + 1, PAGE_SIZE) - 1,
    };
    let heap_area = MemoryArea {
        start: KERNEL_HEAP_BASE,
        end: KERNEL_HEAP_BASE + heap_phys_area.size() - 1,
    };

    boot_map_range(
        root,
        kernel_area,
        kernel_phys_area,
        VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize,
        MemoryAttribute::Normal,
    );
    for index in 0..direct_map_regions.len() {
        let region = direct_map_regions
            .get(index)
            .expect("direct-map region index must be valid");
        let physical_area = region.area();
        boot_map_range(
            root,
            direct_map_virtual_area(physical_area),
            physical_area,
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            region.memory_attribute(),
        );
    }
    boot_map_range(
        root,
        heap_area,
        heap_phys_area,
        VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
        MemoryAttribute::Normal,
    );

    if let Some(initramfs) = initramfs_paddr {
        let initramfs_phys_area = MemoryArea {
            start: align_down(initramfs.start, PAGE_SIZE),
            end: align_up(initramfs.end + 1, PAGE_SIZE) - 1,
        };
        if !direct_map_regions
            .contains_area_with_attribute(initramfs_phys_area, MemoryAttribute::Normal)
        {
            direct_map_regions
                .validate_alias(initramfs_phys_area, MemoryAttribute::Normal)
                .unwrap_or_else(|error| {
                    panic!("initramfs conflicts with direct-map attribute: {}", error)
                });
            boot_map_range(
                root,
                direct_map_virtual_area(initramfs_phys_area),
                initramfs_phys_area,
                VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
                MemoryAttribute::Normal,
            );
        }
    }

    unsafe {
        (*root).switch_for_boot(BOOT_ASID);
    }
}
