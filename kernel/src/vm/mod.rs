//! Virtual memory module.
//!
//! This module provides the virtual memory abstraction for the kernel. It
//! includes functions for managing virtual address spaces.

pub mod addr;
pub mod boot;
pub mod ioremap;
pub mod manager;
pub mod vmem;

pub use addr::{
    PhysAddr, VirtAddr, boot_phys_to_virt, boot_virt_to_phys, finalize_runtime_memory_layout,
    get_boot_hhdm_offset, get_current_direct_map_phys_range, get_heap_phys_layout, get_hhdm_offset,
    phys_to_virt, set_hhdm_offset, transition_kernel_memory_layout, virt_to_phys,
};
pub use ioremap::{ioremap, iounmap};

use manager::VirtualMemoryManager;
use vmem::MemoryArea;
use vmem::VirtualMemoryMap;
use vmem::VirtualMemoryPermission;

use crate::arch::Arch;
use crate::arch::get_kernel_trapvector_paddr;
use crate::arch::set_trapvector;
use crate::arch::vm::alloc_virtual_address_space;
use crate::arch::vm::get_root_pagetable;
use crate::early_println;
use crate::environment::KERNEL_VM_STACK_SIZE;
use crate::environment::KERNEL_VM_STACK_START;
use crate::environment::MAX_NUM_CPUS;
use crate::environment::PAGE_SIZE;
use crate::environment::USER_STACK_END;
use crate::environment::{KERNEL_HEAP_BASE, SCARLET_HHDM_BASE};
use crate::environment::{
    KERNEL_HEAP_SIZE, KERNEL_KSTACK_REGION_END, KERNEL_KSTACK_REGION_START,
    KERNEL_KSTACK_SLOT_SIZE, KERNEL_KSTACK_SLOTS, TASK_KERNEL_STACK_SIZE,
};
use crate::sched::scheduler::get_scheduler;
use crate::task::Task;
use core::sync::atomic::Ordering;
use spin::{Mutex, Once};

extern crate alloc;

static KERNEL_VM_MANAGER: Once<VirtualMemoryManager> = Once::new();
static HHDM_AREA: Once<MemoryArea> = Once::new();
static KERNEL_HEAP_AREA: Once<MemoryArea> = Once::new();
static HHDM_PHYS_AREA: Once<MemoryArea> = Once::new();
static KERNEL_HEAP_PHYS_AREA: Once<MemoryArea> = Once::new();

fn align_down(addr: usize, align: usize) -> usize {
    addr & !(align - 1)
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

pub fn get_kernel_vm_manager() -> &'static VirtualMemoryManager {
    KERNEL_VM_MANAGER.call_once(|| VirtualMemoryManager::new())
}

static KERNEL_AREA: Once<MemoryArea> = Once::new();
/* Initialize MMU and enable paging */
#[allow(static_mut_refs)]
pub fn kernel_vm_init(
    direct_map_paddr: MemoryArea,
    initramfs_paddr: Option<MemoryArea>,
    heap_paddr: MemoryArea,
) {
    let manager = get_kernel_vm_manager();

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: start");

    let asid = alloc_virtual_address_space(); /* Kernel ASID */
    let root_page_table = get_root_pagetable(asid).unwrap();

    manager.set_asid(asid);

    unsafe extern "C" {
        static __KERNEL_SPACE_START: usize;
        static __KERNEL_SPACE_END: usize;
    }

    let kernel_area = MemoryArea {
        start: unsafe { &__KERNEL_SPACE_START as *const usize as usize },
        end: unsafe { &__KERNEL_SPACE_END as *const usize as usize } - 1,
    };
    let kernel_phys_area = MemoryArea {
        start: addr::kernel_virt_to_phys(kernel_area.start),
        end: addr::kernel_virt_to_phys(kernel_area.end),
    };
    let direct_map_phys_area = MemoryArea {
        start: align_down(direct_map_paddr.start, PAGE_SIZE),
        end: align_up(direct_map_paddr.end + 1, PAGE_SIZE) - 1,
    };
    let hhdm_area = MemoryArea {
        start: SCARLET_HHDM_BASE + direct_map_phys_area.start,
        end: SCARLET_HHDM_BASE + direct_map_phys_area.end,
    };
    let heap_phys_area = MemoryArea {
        start: align_down(heap_paddr.start, PAGE_SIZE),
        end: align_up(heap_paddr.end + 1, PAGE_SIZE) - 1,
    };
    let kernel_heap_area = MemoryArea {
        start: KERNEL_HEAP_BASE,
        end: KERNEL_HEAP_BASE + KERNEL_HEAP_SIZE - 1,
    };

    KERNEL_AREA.call_once(|| kernel_area);
    HHDM_AREA.call_once(|| hhdm_area);
    KERNEL_HEAP_AREA.call_once(|| kernel_heap_area);
    HHDM_PHYS_AREA.call_once(|| direct_map_phys_area);
    KERNEL_HEAP_PHYS_AREA.call_once(|| heap_phys_area);

    let kernel_map = VirtualMemoryMap {
        vmarea: kernel_area,
        pmarea: kernel_phys_area,
        vm_start: kernel_area.start,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize,
        is_shared: true, // Kernel memory should be shared across all processes
        owner: None,
    };
    get_kernel_vm_manager()
        .add_memory_map(kernel_map.clone())
        .map_err(|e| panic!("Failed to add kernel memory map: {}", e))
        .unwrap();
    /* Pre-map the kernel space */
    root_page_table
        .map_memory_area(asid, kernel_map, true, true)
        .map_err(|e| panic!("Failed to map kernel memory area: {}", e))
        .unwrap();

    let hhdm_map = VirtualMemoryMap {
        vmarea: hhdm_area,
        pmarea: direct_map_phys_area,
        vm_start: hhdm_area.start,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize,
        is_shared: true,
        owner: None,
    };
    get_kernel_vm_manager()
        .add_memory_map(hhdm_map.clone())
        .map_err(|e| panic!("Failed to add HHDM memory map: {}", e))
        .unwrap();
    root_page_table
        .map_memory_area(asid, hhdm_map, true, true)
        .map_err(|e| panic!("Failed to map HHDM memory area: {}", e))
        .unwrap();

    let heap_map = VirtualMemoryMap {
        vmarea: kernel_heap_area,
        pmarea: heap_phys_area,
        vm_start: kernel_heap_area.start,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize,
        is_shared: true,
        owner: None,
    };
    get_kernel_vm_manager()
        .add_memory_map(heap_map.clone())
        .map_err(|e| panic!("Failed to add heap memory map: {}", e))
        .unwrap();
    root_page_table
        .map_memory_area(asid, heap_map, true, true)
        .map_err(|e| panic!("Failed to map heap memory area: {}", e))
        .unwrap();

    if let Some(initramfs_paddr) = initramfs_paddr {
        let initramfs_phys_area = MemoryArea {
            start: align_down(initramfs_paddr.start, PAGE_SIZE),
            end: align_up(initramfs_paddr.end + 1, PAGE_SIZE) - 1,
        };
        let initramfs_hhdm_area = MemoryArea {
            start: phys_to_virt(initramfs_phys_area.start),
            end: phys_to_virt(initramfs_phys_area.end),
        };
        let initramfs_map = VirtualMemoryMap {
            vmarea: initramfs_hhdm_area,
            pmarea: initramfs_phys_area,
            vm_start: initramfs_hhdm_area.start,
            permissions: VirtualMemoryPermission::Read as usize
                | VirtualMemoryPermission::Write as usize,
            is_shared: true,
            owner: None,
        };
        if initramfs_hhdm_area.start < hhdm_area.start || initramfs_hhdm_area.end > hhdm_area.end {
            get_kernel_vm_manager()
                .add_memory_map(initramfs_map.clone())
                .map_err(|e| panic!("Failed to add initramfs memory map: {}", e))
                .unwrap();
            root_page_table
                .map_memory_area(asid, initramfs_map, true, true)
                .map_err(|e| panic!("Failed to map initramfs memory area: {}", e))
                .unwrap();
        }
    }

    early_println!(
        "Kernel space mapped       : {:#018x} - {:#018x}",
        kernel_area.start,
        kernel_area.end
    );
    early_println!(
        "HHDM mapped               : {:#018x} - {:#018x}",
        hhdm_area.start,
        hhdm_area.end
    );
    early_println!(
        "Kernel heap mapped        : {:#018x} - {:#018x}",
        kernel_heap_area.start,
        kernel_heap_area.end
    );

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: setup_trampoline_for_kernel...");

    crate::arch::vm::setup_trampoline_for_kernel(get_kernel_vm_manager());

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: trampoline ok");

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: switch (ttbr0/arch-dependent)...");
    root_page_table.switch(manager.get_asid());

    // Initialize the ioremap subsystem now that the kernel VM manager and heap
    // are ready.  Device drivers call ioremap() to map their MMIO regions
    // dynamically instead of relying on a static identity mapping.
    ioremap::ioremap_init();

    early_println!(
        "IOREMAP region            : {:#018x} - {:#018x}",
        crate::environment::IOREMAP_START,
        crate::environment::IOREMAP_END,
    );

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: done");

    finalize_runtime_memory_layout();
}

pub fn user_vm_init(task: &Task) {
    let asid = alloc_virtual_address_space();
    task.vm_manager.set_asid(asid);

    /* User stack page */
    let num_of_stack_page = 256; // 1MB user stack (4KB pages)
    let stack_start = USER_STACK_END - num_of_stack_page * PAGE_SIZE;
    task.allocate_stack_pages(stack_start, num_of_stack_page)
        .map_err(|e| panic!("Failed to allocate user stack pages: {}", e))
        .unwrap();

    /* Guard page */
    task.allocate_guard_pages(stack_start - PAGE_SIZE, 1)
        .map_err(|e| panic!("Failed to allocate guard page: {}", e))
        .unwrap();

    crate::arch::vm::setup_trampoline_for_user(&task.vm_manager);

    // Trampoline-managed high-VA infrastructure also includes per-task kstack windows.
    // Keep this in the VM init flow so callers don't need a separate map_* step.
    setup_trampoline_for_task_kstack_window(task)
        .map_err(|e| panic!("Failed to setup task kstack window: {}", e))
        .unwrap();
}

pub fn user_kernel_vm_init(task: &Task) {
    let asid = alloc_virtual_address_space();
    let root_page_table = get_root_pagetable(asid).unwrap();
    task.vm_manager.set_asid(asid);

    let kernel_area = *KERNEL_AREA.get().expect("KERNEL_AREA not initialized");
    let hhdm_area = *HHDM_AREA.get().expect("HHDM_AREA not initialized");
    let kernel_heap_area = *KERNEL_HEAP_AREA
        .get()
        .expect("KERNEL_HEAP_AREA not initialized");
    let hhdm_phys_area = *HHDM_PHYS_AREA
        .get()
        .expect("HHDM_PHYS_AREA not initialized");
    let kernel_heap_phys_area = *KERNEL_HEAP_PHYS_AREA
        .get()
        .expect("KERNEL_HEAP_PHYS_AREA not initialized");

    let kernel_map = VirtualMemoryMap {
        vmarea: kernel_area,
        pmarea: MemoryArea::new(
            addr::kernel_virt_to_phys(kernel_area.start),
            addr::kernel_virt_to_phys(kernel_area.end),
        ),
        vm_start: kernel_area.start,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize,
        is_shared: true, // Kernel memory should be shared across all processes
        owner: None,
    };
    task.vm_manager
        .add_memory_map(kernel_map.clone())
        .map_err(|e| {
            panic!("Failed to add kernel memory map: {}", e);
        })
        .unwrap();
    /* Pre-map the kernel space */
    root_page_table
        .map_memory_area(asid, kernel_map, true, true)
        .map_err(|e| {
            panic!("Failed to map kernel memory area: {}", e);
        })
        .unwrap();

    let hhdm_map = VirtualMemoryMap {
        vmarea: hhdm_area,
        pmarea: hhdm_phys_area,
        vm_start: hhdm_area.start,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize,
        is_shared: true,
        owner: None,
    };
    task.vm_manager
        .add_memory_map(hhdm_map.clone())
        .map_err(|e| panic!("Failed to add HHDM memory map: {}", e))
        .unwrap();
    root_page_table
        .map_memory_area(asid, hhdm_map, true, true)
        .map_err(|e| panic!("Failed to map HHDM memory area: {}", e))
        .unwrap();

    let heap_map = VirtualMemoryMap {
        vmarea: kernel_heap_area,
        pmarea: kernel_heap_phys_area,
        vm_start: kernel_heap_area.start,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize,
        is_shared: true,
        owner: None,
    };
    task.vm_manager
        .add_memory_map(heap_map.clone())
        .map_err(|e| panic!("Failed to add heap memory map: {}", e))
        .unwrap();
    root_page_table
        .map_memory_area(asid, heap_map, true, true)
        .map_err(|e| panic!("Failed to map heap memory area: {}", e))
        .unwrap();
    task.data_size.store(kernel_area.end + 1, Ordering::SeqCst);

    /* Stack page */
    task.allocate_stack_pages(KERNEL_VM_STACK_START, KERNEL_VM_STACK_SIZE / PAGE_SIZE)
        .map_err(|e| panic!("Failed to allocate kernel stack pages: {}", e))
        .unwrap();

    crate::arch::vm::setup_trampoline_for_user(&task.vm_manager);

    setup_trampoline_for_task_kstack_window(task)
        .map_err(|e| panic!("Failed to setup task kstack window: {}", e))
        .unwrap();
}

// --------------------
// Kernel stack window allocator (shared kernel PT)
// --------------------

struct KernelKstackAllocator {
    slots: alloc::vec::Vec<bool>,
}

impl KernelKstackAllocator {
    fn new() -> Self {
        Self {
            slots: alloc::vec![false; KERNEL_KSTACK_SLOTS],
        }
    }

    fn alloc_slot(&mut self) -> Option<(usize, usize, usize)> {
        for (idx, used) in self.slots.iter_mut().enumerate() {
            if !*used {
                *used = true;
                let base = KERNEL_KSTACK_REGION_START + idx * KERNEL_KSTACK_SLOT_SIZE;
                let top = base + KERNEL_KSTACK_SLOT_SIZE; // exclusive top
                return Some((idx, base, top));
            }
        }
        None
    }

    fn free_slot(&mut self, idx: usize) {
        if idx < self.slots.len() {
            self.slots[idx] = false;
        }
    }

    fn slot_index_for_base(&self, base: usize) -> Option<usize> {
        if base < KERNEL_KSTACK_REGION_START || base > KERNEL_KSTACK_REGION_END {
            return None;
        }
        let off = base - KERNEL_KSTACK_REGION_START;
        if off % KERNEL_KSTACK_SLOT_SIZE != 0 {
            return None;
        }
        Some(off / KERNEL_KSTACK_SLOT_SIZE)
    }
}

static KSTACK_ALLOC_ONCE: Once<Mutex<KernelKstackAllocator>> = Once::new();

fn kstack_alloc() -> &'static Mutex<KernelKstackAllocator> {
    KSTACK_ALLOC_ONCE.call_once(|| Mutex::new(KernelKstackAllocator::new()))
}

/// Map the task's kernel stack physical pages into the shared kernel PT at a unique high VA window.
/// Adds an unmapped guard page at the bottom of the window.
#[allow(static_mut_refs)]
pub fn setup_trampoline_for_task_kstack_window(task: &Task) -> Result<(), &'static str> {
    // Allocate a window slot
    let (slot_idx, base, _top) = kstack_alloc()
        .lock()
        .alloc_slot()
        .ok_or("No free kernel stack window slots")?;

    // Physical (identity) address range of the task's kernel stack
    let km_area = task.get_kernel_stack_memory_area_paddr();
    let paddr_start = km_area.start;
    let paddr_end = km_area.end;

    // Ensure page alignment
    if paddr_start % PAGE_SIZE != 0 || (paddr_end + 1 - paddr_start) % PAGE_SIZE != 0 {
        return Err("Kernel stack memory area is not page-aligned");
    }

    // Virtual window (skip guard page at the bottom)
    let vaddr_start = base + crate::environment::PAGE_SIZE;
    let vaddr_end = vaddr_start + TASK_KERNEL_STACK_SIZE - 1;

    // Map into shared kernel PT
    let kman = get_kernel_vm_manager();
    let mmap = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: vaddr_start,
            end: vaddr_end,
        },
        pmarea: MemoryArea {
            start: paddr_start,
            end: paddr_end,
        },
        vm_start: vaddr_start,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize,
        is_shared: true,
        owner: None,
    };

    kman.add_memory_map(mmap.clone())
        .map_err(|_| "Failed to add kernel stack mmap")?;
    let root = kman
        .get_root_page_table()
        .ok_or("Kernel root page table not set")?;
    root.map_memory_area(kman.get_asid(), mmap, true, true)
        .map_err(|_| "Failed to map kernel stack window")?;

    // Record base for later SP and teardown
    task.set_kernel_stack_window_base(Some((slot_idx, base)));

    // Update the task's kernel context SP to point into the high-VA window.
    // After boot, tasks are scheduled via `switch_to` which restores KernelContext.sp.
    // If we keep SP pointing to the raw allocated stack pointer, AArch64 can fault at
    // exception entry (SP_EL1) and the kernel may also miss the intended trampoline-managed
    // stack window. The window top is page-aligned, so stack alignment is also guaranteed.
    let stack_top = (base + crate::environment::PAGE_SIZE + TASK_KERNEL_STACK_SIZE) as u64;
    let tf_size = core::mem::size_of::<crate::arch::Trapframe>() as u64;
    let tf_align = core::mem::align_of::<crate::arch::Trapframe>() as u64;
    debug_assert!(tf_align.is_power_of_two());
    let sp = (stack_top - tf_size) & !(tf_align - 1);
    task.with_kernel_context(|kctx| {
        kctx.set_sp(sp);
    });

    #[cfg(any(debug_assertions, test))]
    crate::println!(
        "Mapped kernel stack window for Task (allocating): slot {} {:#x} - {:#x}",
        slot_idx,
        base,
        base + KERNEL_KSTACK_SLOT_SIZE - 1
    );

    // NOTE: vcpu.sp (user sp) is set separately; this is kernel SP for `switch_to`/traps.

    // Debug verification in test / debug builds: ensure guard page is unmapped
    #[cfg(any(debug_assertions, test))]
    {
        if verify_task_kernel_stack_guard(task) {
            early_println!("Kernel stack guard OK (slot {})", slot_idx);
        } else {
            early_println!(
                "WARN: Kernel stack guard mapping anomaly (slot {})",
                slot_idx
            );
        }
    }
    Ok(())
}

/// Unmap and free the task's kernel stack window from the shared kernel PT.
#[allow(static_mut_refs)]
pub fn teardown_trampoline_for_task_kstack_window(task: &mut Task) {
    if let Some((slot_idx, base)) = task.get_kernel_stack_window_base() {
        let vstart = base + crate::environment::PAGE_SIZE;
        let vend = vstart + TASK_KERNEL_STACK_SIZE - 1;

        // Remove from kernel VM manager and unmap pages
        let kman = get_kernel_vm_manager();
        let asid = kman.get_asid();
        if let Some(root) = kman.get_root_page_table() {
            let mut v = vstart;
            while v <= vend {
                root.unmap(asid, v);
                v += PAGE_SIZE;
            }
        }
        // Best-effort remove VMA entries
        let mut v = vstart;
        while v <= vend {
            let _ = kman.remove_memory_map_by_addr(v);
            v += PAGE_SIZE;
        }
        // Free slot
        kstack_alloc().lock().free_slot(slot_idx);
        task.set_kernel_stack_window_base(None);
    }
}

/// Verify that a task's kernel stack guard page is unmapped and stack pages are mapped.
/// Returns true if the guard page has no associated memory map and a sample stack address is mapped.
pub fn verify_task_kernel_stack_guard(task: &Task) -> bool {
    let (slot_idx, base) = match task.get_kernel_stack_window_base() {
        Some(v) => v,
        None => return false,
    };
    let guard_start = base;
    let guard_sample = guard_start; // Any address in guard page
    let stack_first = base + PAGE_SIZE; // First mapped byte of stack window
    let stack_sample = stack_first + (PAGE_SIZE / 2); // Sample inside first page

    let kman = get_kernel_vm_manager();
    let guard_map = kman.search_memory_map(guard_sample);
    let stack_map = kman.search_memory_map(stack_sample);

    let guard_ok = guard_map.is_none();
    let stack_ok = stack_map
        .map(|m| m.vmarea.start <= stack_sample && stack_sample <= m.vmarea.end)
        .unwrap_or(false);

    if !(guard_ok && stack_ok) {
        early_println!(
            "[verify_kstack_guard] slot {} guard_ok={} stack_ok={} guard_map_start={:?}",
            slot_idx,
            guard_ok,
            stack_ok,
            guard_map.map(|m| m.vmarea.start)
        );
    }
    guard_ok && stack_ok
}

pub fn setup_user_stack(task: &Task) -> (usize, usize) {
    /* User stack page */
    let num_of_stack_page = 256; // 1MB user stack (4KB pages)
    let stack_base = USER_STACK_END - num_of_stack_page * PAGE_SIZE;
    task.allocate_stack_pages(stack_base, num_of_stack_page)
        .map_err(|e| panic!("Failed to allocate user stack pages: {}", e))
        .unwrap();
    /* Guard page */
    task.allocate_guard_pages(stack_base - PAGE_SIZE, 1)
        .map_err(|e| panic!("Failed to allocate guard page: {}", e))
        .unwrap();

    (stack_base, USER_STACK_END)
}

static TRAMPOLINE_TRAP_VECTOR: Once<usize> = Once::new();
static TRAMPOLINE_ARCH: Mutex<[Option<usize>; MAX_NUM_CPUS]> = Mutex::new([None; MAX_NUM_CPUS]);

pub fn set_trampoline_trap_vector(trap_vector: usize) {
    TRAMPOLINE_TRAP_VECTOR.call_once(|| trap_vector);
}

pub fn get_trampoline_trap_vector() -> usize {
    *TRAMPOLINE_TRAP_VECTOR
        .get()
        .expect("Trampoline is not initialized")
}

pub fn get_guest_trapvector_trampoline() -> usize {
    let trampoline_base = get_trampoline_trap_vector();
    let user_entry = crate::arch::get_user_trapvector_paddr();
    let guest_entry = crate::arch::get_guest_trapvector_paddr();
    let offset = guest_entry.wrapping_sub(user_entry);
    trampoline_base.wrapping_add(offset)
}

pub fn set_trampoline_arch(cpu_id: usize, arch: usize) {
    let mut trampolines = TRAMPOLINE_ARCH.lock();
    trampolines[cpu_id] = Some(arch);
}

pub fn get_trampoline_arch(cpu_id: usize) -> usize {
    let trampolines = TRAMPOLINE_ARCH.lock();
    trampolines[cpu_id].expect("Trampoline is not initialized")
}

pub fn switch_to_kernel_vm() {
    let manager = get_kernel_vm_manager();
    let root_page_table = manager
        .get_root_page_table()
        .expect("Root page table is not set");
    set_trapvector(get_kernel_trapvector_paddr());
    root_page_table.switch(manager.get_asid());
}

pub fn switch_to_user_vm(cpu: &mut Arch) {
    let cpu_id = cpu.get_cpuid();
    let task = get_scheduler()
        .get_current_task(cpu_id)
        .expect("No current task found");
    let manager = &task.vm_manager;
    let root_page_table = manager
        .get_root_page_table()
        .expect("Root page table is not set");
    set_trapvector(get_trampoline_trap_vector());
    root_page_table.switch(manager.get_asid());
}
