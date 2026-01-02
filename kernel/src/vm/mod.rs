//! Virtual memory module.
//!
//! This module provides the virtual memory abstraction for the kernel. It
//! includes functions for managing virtual address spaces.

use manager::VirtualMemoryManager;
use vmem::MemoryArea;
use vmem::VirtualMemoryMap;
use vmem::VirtualMemoryPermission;

use crate::arch::Arch;
use crate::arch::get_device_memory_areas;
use crate::arch::get_kernel_trapvector_paddr;
use crate::arch::set_trapvector;
use crate::arch::vm::alloc_virtual_address_space;
use crate::arch::vm::get_root_pagetable;
use crate::early_println;
use crate::environment::KERNEL_VM_STACK_SIZE;
use crate::environment::KERNEL_VM_STACK_START;
use crate::environment::NUM_OF_CPUS;
use crate::environment::PAGE_SIZE;
use crate::environment::USER_STACK_END;
use crate::environment::{
    KERNEL_KSTACK_REGION_END, KERNEL_KSTACK_REGION_START, KERNEL_KSTACK_SLOT_SIZE,
    KERNEL_KSTACK_SLOTS, TASK_KERNEL_STACK_SIZE,
};
use crate::sched::scheduler::get_scheduler;
use crate::task::Task;
use spin::{Mutex, Once};

extern crate alloc;

pub mod manager;
pub mod vmem;

static mut KERNEL_VM_MANAGER: Option<VirtualMemoryManager> = None;

pub fn get_kernel_vm_manager() -> &'static mut VirtualMemoryManager {
    unsafe {
        match KERNEL_VM_MANAGER {
            Some(ref mut m) => m,
            None => {
                kernel_vm_manager_init();
                get_kernel_vm_manager()
            }
        }
    }
}

fn kernel_vm_manager_init() {
    let manager = VirtualMemoryManager::new();

    unsafe {
        KERNEL_VM_MANAGER = Some(manager);
    }
}

static mut KERNEL_AREA: Option<MemoryArea> = None;
/* Initialize MMU and enable paging */
#[allow(static_mut_refs)]
pub fn kernel_vm_init(kernel_area: MemoryArea) {
    let manager = get_kernel_vm_manager();

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: start");

    let asid = alloc_virtual_address_space(); /* Kernel ASID */
    let root_page_table = get_root_pagetable(asid).unwrap();

    manager.set_asid(asid);

    /* Map kernel space */
    let kernel_start = kernel_area.start;
    let kernel_end = kernel_area.end;

    let kernel_area = MemoryArea {
        start: kernel_start,
        end: kernel_end,
    };
    unsafe {
        KERNEL_AREA = Some(kernel_area);
    }

    let kernel_map = VirtualMemoryMap {
        vmarea: kernel_area,
        pmarea: kernel_area,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize,
        is_shared: true, // Kernel memory should be shared across all processes
        owner: None,
    };
    manager
        .add_memory_map(kernel_map.clone())
        .map_err(|e| panic!("Failed to add kernel memory map: {}", e))
        .unwrap();
    /* Pre-map the kernel space */
    root_page_table
        .map_memory_area(asid, kernel_map, true, true)
        .map_err(|e| panic!("Failed to map kernel memory area: {}", e))
        .unwrap();

    // Map device memory areas (architecture-specific)
    for dev_area in get_device_memory_areas() {
        let dev_map = VirtualMemoryMap {
            vmarea: dev_area,
            pmarea: dev_area,
            permissions: VirtualMemoryPermission::Read as usize
                | VirtualMemoryPermission::Write as usize,
            is_shared: true, // Device memory should be shared
            owner: None,
        };
        manager
            .add_memory_map(dev_map.clone())
            .map_err(|e| panic!("Failed to add device memory map: {}", e))
            .unwrap();
        root_page_table
            .map_memory_area(asid, dev_map.clone(), true, true)
            .map_err(|e| panic!("Failed to map device memory area: {}", e))
            .unwrap();
    }

    early_println!(
        "Kernel space mapped       : {:#018x} - {:#018x}",
        kernel_area.start,
        kernel_area.end
    );
    for dev_area in get_device_memory_areas() {
        early_println!(
            "Device space mapped       : {:#018x} - {:#018x}",
            dev_area.start,
            dev_area.end
        );
    }

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: setup_trampoline_for_kernel...");

    crate::arch::vm::setup_trampoline_for_kernel(manager);

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: trampoline ok");

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: switch (ttbr0/arch-dependent)...");
    root_page_table.switch(manager.get_asid());

    #[cfg(any(debug_assertions, test))]
    early_println!("[vm] kernel_vm_init: done");
}

pub fn user_vm_init(task: &mut Task) {
    let asid = alloc_virtual_address_space();
    task.vm_manager.set_asid(asid);

    /* User stack page */
    let num_of_stack_page = 16; // 16 pages for user stack
    let stack_start = USER_STACK_END - num_of_stack_page * PAGE_SIZE;
    task.allocate_stack_pages(stack_start, num_of_stack_page)
        .map_err(|e| panic!("Failed to allocate user stack pages: {}", e))
        .unwrap();

    /* Guard page */
    task.allocate_guard_pages(stack_start - PAGE_SIZE, 1)
        .map_err(|e| panic!("Failed to allocate guard page: {}", e))
        .unwrap();

    crate::arch::vm::setup_trampoline_for_user(&mut task.vm_manager);

    // Trampoline-managed high-VA infrastructure also includes per-task kstack windows.
    // Keep this in the VM init flow so callers don't need a separate map_* step.
    setup_trampoline_for_task_kstack_window(task)
        .map_err(|e| panic!("Failed to setup task kstack window: {}", e))
        .unwrap();
}

pub fn user_kernel_vm_init(task: &mut Task) {
    let asid = alloc_virtual_address_space();
    let root_page_table = get_root_pagetable(asid).unwrap();
    task.vm_manager.set_asid(asid);

    let kernel_area = unsafe { KERNEL_AREA.unwrap() };

    let kernel_map = VirtualMemoryMap {
        vmarea: kernel_area,
        pmarea: kernel_area,
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
    task.data_size = kernel_area.end + 1;

    /* Stack page */
    task.allocate_stack_pages(KERNEL_VM_STACK_START, KERNEL_VM_STACK_SIZE / PAGE_SIZE)
        .map_err(|e| panic!("Failed to allocate kernel stack pages: {}", e))
        .unwrap();

    // Map device memory areas (architecture-specific)
    for dev_area in get_device_memory_areas() {
        let dev_map = VirtualMemoryMap {
            vmarea: dev_area,
            pmarea: dev_area,
            permissions: VirtualMemoryPermission::Read as usize
                | VirtualMemoryPermission::Write as usize,
            is_shared: true, // Device memory should be shared
            owner: None,
        };
        task.vm_manager
            .add_memory_map(dev_map)
            .map_err(|e| panic!("Failed to add device memory map: {}", e))
            .unwrap();
    }

    crate::arch::vm::setup_trampoline_for_user(&mut task.vm_manager);

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
pub fn setup_trampoline_for_task_kstack_window(task: &mut Task) -> Result<(), &'static str> {
    // Allocate a window slot
    let (slot_idx, base, _top) = kstack_alloc()
        .lock()
        .alloc_slot()
        .ok_or("No free kernel stack window slots")?;

    // Physical (identity) address range of the task's kernel stack
    let km_area = task.get_kernel_stack_memory_area_paddr();
    let paddr_start = km_area.start;
    let paddr_end = paddr_start + TASK_KERNEL_STACK_SIZE - 1;

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
    task.get_kernel_context_mut().set_sp(sp);

    crate::early_println!(
        "Mapped kernel stack window for Task {}: slot {} {:#x} - {:#x}",
        task.get_id(),
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

pub fn setup_user_stack(task: &mut Task) -> (usize, usize) {
    /* User stack page */
    let num_of_stack_page = 16; // 4 pages for user stack
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

static mut TRAMPOLINE_TRAP_VECTOR: Option<usize> = None;
static mut TRAMPOLINE_ARCH: [Option<usize>; NUM_OF_CPUS] = [None; NUM_OF_CPUS];

pub fn set_trampoline_trap_vector(trap_vector: usize) {
    unsafe {
        TRAMPOLINE_TRAP_VECTOR = Some(trap_vector);
    }
}

pub fn get_trampoline_trap_vector() -> usize {
    unsafe {
        match TRAMPOLINE_TRAP_VECTOR {
            Some(v) => v,
            None => panic!("Trampoline is not initialized"),
        }
    }
}

pub fn set_trampoline_arch(cpu_id: usize, arch: usize) {
    unsafe {
        TRAMPOLINE_ARCH[cpu_id] = Some(arch);
    }
}

pub fn get_trampoline_arch(cpu_id: usize) -> usize {
    unsafe {
        match TRAMPOLINE_ARCH[cpu_id] {
            Some(v) => v,
            None => panic!("Trampoline is not initialized"),
        }
    }
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
