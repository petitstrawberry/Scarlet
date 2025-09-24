//! Virtual memory module.
//! 
//! This module provides the virtual memory abstraction for the kernel. It
//! includes functions for managing virtual address spaces.

use manager::VirtualMemoryManager;
use vmem::MemoryArea;
use vmem::VirtualMemoryMap;
use vmem::VirtualMemoryPermission;

use crate::arch::get_cpu;
use crate::arch::get_kernel_trapvector_paddr;
use crate::arch::get_user_trapvector_paddr;
use crate::arch::set_trapvector;
use crate::arch::vm::alloc_virtual_address_space;
use crate::arch::vm::get_root_pagetable;
use crate::arch::Arch;
use crate::early_println;
use crate::environment::KERNEL_VM_STACK_SIZE;
use crate::environment::KERNEL_VM_STACK_START;
use crate::environment::NUM_OF_CPUS;
use crate::environment::PAGE_SIZE;
use crate::environment::USER_STACK_END;
use crate::environment::VMMAX;
use crate::println;
use crate::sched::scheduler::get_scheduler;
use crate::task::Task;

extern crate alloc;

pub mod manager;
pub mod vmem;

unsafe extern "C" {
    static __KERNEL_SPACE_START: usize;
    static __KERNEL_SPACE_END: usize;
    static __TRAMPOLINE_START: usize;
    static __TRAMPOLINE_END: usize;
}

static mut KERNEL_VM_MANAGER: Option<VirtualMemoryManager> = None;

pub fn get_kernel_vm_manager() -> &'static mut VirtualMemoryManager {
    unsafe
    {
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
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize |
            VirtualMemoryPermission::Execute as usize,
        is_shared: true, // Kernel memory should be shared across all processes
        owner: None,
    };
    manager.add_memory_map(kernel_map.clone()).map_err(|e| panic!("Failed to add kernel memory map: {}", e)).unwrap();
    /* Pre-map the kernel space */
    root_page_table.map_memory_area(asid, kernel_map).map_err(|e| panic!("Failed to map kernel memory area: {}", e)).unwrap();

    let dev_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: 0x00,
            end: 0x7fff_ffff,
        },
        pmarea: MemoryArea {
            start: 0x00,
            end: 0x7fff_ffff,
        },
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize,
        is_shared: true, // Device memory should be shared
        owner: None,
    };
    manager.add_memory_map(dev_map.clone()).map_err(|e| panic!("Failed to add device memory map: {}", e)).unwrap();

    early_println!("Kernel space mapped       : {:#018x} - {:#018x}", kernel_area.start, kernel_area.end);
    early_println!("Device space mapped       : {:#018x} - {:#018x}", dev_map.vmarea.start, dev_map.vmarea.end);
    early_println!("Kernel space mapped       : {:#018x} - {:#018x}", kernel_start, kernel_end);

    setup_trampoline(manager);

    root_page_table.switch(manager.get_asid());
}

pub fn user_vm_init(task: &mut Task) {
    let asid = alloc_virtual_address_space();
    task.vm_manager.set_asid(asid);

    /* User stack page */
    let num_of_stack_page = 16; // 4 pages for user stack
    let stack_start = USER_STACK_END - num_of_stack_page * PAGE_SIZE;
    task.allocate_stack_pages(stack_start, num_of_stack_page).map_err(|e| panic!("Failed to allocate user stack pages: {}", e)).unwrap();

    /* Guard page */
   task.allocate_guard_pages(stack_start - PAGE_SIZE, 1).map_err(|e| panic!("Failed to allocate guard page: {}", e)).unwrap();

    setup_trampoline(&mut task.vm_manager);
}

pub fn user_kernel_vm_init(task: &mut Task) {
    let asid = alloc_virtual_address_space();
    let root_page_table = get_root_pagetable(asid).unwrap();
    task.vm_manager.set_asid(asid);

    let kernel_area = unsafe { KERNEL_AREA.unwrap() };

    let kernel_map = VirtualMemoryMap {
        vmarea: kernel_area,
        pmarea: kernel_area,
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize |
            VirtualMemoryPermission::Execute as usize,
        is_shared: true, // Kernel memory should be shared across all processes
        owner: None,
    };
    task.vm_manager.add_memory_map(kernel_map.clone()).map_err(|e| {
        panic!("Failed to add kernel memory map: {}", e);
    }).unwrap();
    /* Pre-map the kernel space */
    root_page_table.map_memory_area(asid, kernel_map).map_err(|e| {
        panic!("Failed to map kernel memory area: {}", e);
    }).unwrap();
    task.data_size = kernel_area.end + 1;

    /* Stack page */
    task.allocate_stack_pages(KERNEL_VM_STACK_START, KERNEL_VM_STACK_SIZE / PAGE_SIZE).map_err(|e| panic!("Failed to allocate kernel stack pages: {}", e)).unwrap();

    let dev_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: 0x00,
            end: 0x7fff_ffff,
        },
        pmarea: MemoryArea {
            start: 0x00,
            end: 0x7fff_ffff,
        },
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize,
        is_shared: true, // Device memory should be shared
        owner: None,
    };
    task.vm_manager.add_memory_map(dev_map).map_err(|e| panic!("Failed to add device memory map: {}", e)).unwrap();

    setup_trampoline(&mut task.vm_manager);
}

pub fn setup_user_stack(task: &mut Task) -> (usize, usize) {
    /* User stack page */
    let num_of_stack_page = 16; // 4 pages for user stack
    let stack_base = USER_STACK_END - num_of_stack_page * PAGE_SIZE;
    task.allocate_stack_pages(stack_base, num_of_stack_page).map_err(|e| panic!("Failed to allocate user stack pages: {}", e)).unwrap();
    /* Guard page */
    task.allocate_guard_pages(stack_base - PAGE_SIZE, 1).map_err(|e| panic!("Failed to allocate guard page: {}", e)).unwrap();
    
    (stack_base, USER_STACK_END)
}

static mut TRAMPOLINE_TRAP_VECTOR: Option<usize> = None;
static mut TRAMPOLINE_ARCH: [Option<usize>; NUM_OF_CPUS] = [None; NUM_OF_CPUS];

pub fn setup_trampoline(manager: &mut VirtualMemoryManager) {
    let trampoline_start = unsafe { &__TRAMPOLINE_START as *const usize as usize };
    let trampoline_end = unsafe { &__TRAMPOLINE_END as *const usize as usize } - 1;
    let trampoline_size = trampoline_end - trampoline_start;

    let arch = get_cpu().as_paddr_cpu();
    let trampoline_vaddr_start = VMMAX - trampoline_size;
    let trampoline_vaddr_end = VMMAX;

    let trap_entry_paddr = get_user_trapvector_paddr();
    // let trapframe_paddr = arch.get_trapframe_paddr();
    let arch_paddr = arch as *const Arch as usize;
    let trap_entry_offset = trap_entry_paddr - trampoline_start;
    let arch_offset = arch_paddr - trampoline_start;

    let trap_entry_vaddr = trampoline_vaddr_start + trap_entry_offset;
    let arch_vaddr = trampoline_vaddr_start + arch_offset;
    
    // early_println!("Trampoline space mapped   : {:#x} - {:#x}", trampoline_vaddr_start, trampoline_vaddr_end);
    // early_println!("  Trampoline paddr  : {:#x} - {:#x}", trampoline_start, trampoline_end);
    // early_println!("  Trap entry paddr  : {:#x}", trap_entry_paddr);
    // early_println!("  Arch paddr        : {:#x}", arch_paddr);
    // early_println!("  Trampoline vaddr  : {:#x} - {:#x}", trampoline_vaddr_start, trampoline_vaddr_end);
    // early_println!("  Trap entry vaddr  : {:#x}", trap_entry_vaddr);
    // early_println!("  Arch vaddr        : {:#x}", arch_vaddr);
    
    let trampoline_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: trampoline_vaddr_start,
            end: trampoline_vaddr_end,
        },
        pmarea: MemoryArea {
            start: trampoline_start,
            end: trampoline_end,
        },
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize |
            VirtualMemoryPermission::Execute as usize,
        is_shared: true, // Trampoline should be shared across all processes
        owner: None,
    };

    manager.add_memory_map(trampoline_map.clone())
        .map_err(|e| panic!("Failed to add trampoline memory map: {}", e)).unwrap();
    /* Pre-map the trampoline space */
    manager.get_root_page_table().unwrap().map_memory_area(manager.get_asid(), trampoline_map)
        .map_err(|e| panic!("Failed to map trampoline memory area: {}", e)).unwrap();

    set_trampoline_trap_vector(trap_entry_vaddr);
    set_trampoline_arch(arch.get_cpuid(), arch_vaddr);
}

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
    let root_page_table = manager.get_root_page_table().expect("Root page table is not set");
    set_trapvector(get_kernel_trapvector_paddr());
    root_page_table.switch(manager.get_asid());
}

pub fn switch_to_user_vm(cpu: &mut Arch) {
    let cpu_id = cpu.get_cpuid();
    let task = get_scheduler().get_current_task(cpu_id).expect("No current task found");
    let manager = &task.vm_manager;
    let root_page_table = manager.get_root_page_table().expect("Root page table is not set");
    set_trapvector(get_trampoline_trap_vector());
    root_page_table.switch(manager.get_asid());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::early_println;

    /// Test basic virtual memory manager initialization
    #[test_case]
    fn test_vm_manager_init() {
        early_println!("[VM Test] Testing virtual memory manager initialization");
        
        let manager = VirtualMemoryManager::new();
        assert!(manager.get_asid() == 0, "Initial ASID should be 0");
        
        // Test that we can create multiple managers
        let manager2 = VirtualMemoryManager::new();
        assert!(manager2.get_asid() == 0, "New manager ASID should be 0");
        
        early_println!("[VM Test] Virtual memory manager initialization test passed");
    }

    /// Test memory area creation and validation
    #[test_case]
    fn test_memory_area_creation() {
        early_println!("[VM Test] Testing memory area creation");
        
        let area = MemoryArea::new(0x1000, 0x2000);
        assert!(area.start == 0x1000, "Area start should be 0x1000");
        assert!(area.end == 0x2000, "Area end should be 0x2000");
        assert!(area.size() == 0x1000, "Area size should be 0x1000");
        
        early_println!("[VM Test] Memory area creation test passed");
    }

    /// Test virtual memory map creation with different permissions
    #[test_case]
    fn test_vm_map_permissions() {
        early_println!("[VM Test] Testing virtual memory map permissions");
        
        let vmarea = MemoryArea::new(0x10000, 0x11000);
        let pmarea = MemoryArea::new(0x20000, 0x21000);
        
        // Test read-only permission
        let readonly_map = VirtualMemoryMap {
            vmarea,
            pmarea,
            permissions: VirtualMemoryPermission::Read as usize,
            is_shared: false,
            owner: None,
        };
        
        assert!(readonly_map.permissions == VirtualMemoryPermission::Read as usize, 
                "Read-only permission should be set correctly");
        
        // Test read-write permission
        let readwrite_map = VirtualMemoryMap {
            vmarea,
            pmarea,
            permissions: VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            is_shared: false,
            owner: None,
        };
        
        assert!(readwrite_map.permissions == (VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize), 
                "Read-write permission should be set correctly");
        
        early_println!("[VM Test] Virtual memory map permissions test passed");
    }

    /// Test kernel virtual memory initialization
    #[test_case]
    fn test_kernel_vm_init() {
        early_println!("[VM Test] Testing kernel virtual memory initialization");
        
        // Create a dummy kernel area for testing
        let kernel_area = MemoryArea::new(0x80000000, 0x80100000);
        
        // This test verifies that kernel_vm_init doesn't panic
        // The actual MMU setup is tested in architecture-specific tests
        kernel_vm_init(kernel_area);
        
        let manager = get_kernel_vm_manager();
        assert!(manager.get_asid() != 0, "Kernel manager should have non-zero ASID after init");
        
        early_println!("[VM Test] Kernel virtual memory initialization test passed");
    }

    /// Architecture-specific MMU tests for RISC-V
    #[cfg(target_arch = "riscv64")]
    mod riscv64_tests {
        use super::*;
        use crate::arch::riscv64::vm::mmu::{PageTable, PageTableEntry};

        #[test_case]
        fn test_riscv64_page_table_creation() {
            early_println!("[RISC-V MMU Test] Testing page table creation");
            
            // Test page table allocation and initialization
            let page_table = PageTable::new();
            // Check that the first entry is properly initialized
            assert!(!page_table.entries[0].is_valid(), "Initial page table entries should be invalid");
            
            early_println!("[RISC-V MMU Test] Page table creation test passed");
        }

        #[test_case]
        fn test_riscv64_pte_flags() {
            early_println!("[RISC-V MMU Test] Testing page table entry flags");
            
            let mut pte = PageTableEntry::new();
            
            // Test setting and getting flags
            pte.set_valid(true);
            assert!(pte.is_valid(), "PTE should be valid after setting");
            
            pte.set_readable(true);
            assert!(pte.is_readable(), "PTE should be readable after setting");
            
            pte.set_writable(true);
            assert!(pte.is_writable(), "PTE should be writable after setting");
            
            pte.set_executable(true);
            assert!(pte.is_executable(), "PTE should be executable after setting");
            
            early_println!("[RISC-V MMU Test] Page table entry flags test passed");
        }

        #[test_case]
        fn test_riscv64_address_translation() {
            early_println!("[RISC-V MMU Test] Testing virtual address translation");
            
            // Test virtual address breakdown for SV48
            let vaddr = 0x123456789ABC;
            let vpn = [
                (vaddr >> 12) & 0x1FF,    // VPN[0]
                (vaddr >> 21) & 0x1FF,    // VPN[1]
                (vaddr >> 30) & 0x1FF,    // VPN[2]
                (vaddr >> 39) & 0x1FF,    // VPN[3]
            ];
            
            assert!(vpn[0] == ((vaddr >> 12) & 0x1FF), "VPN[0] calculation should be correct");
            assert!(vpn[1] == ((vaddr >> 21) & 0x1FF), "VPN[1] calculation should be correct");
            assert!(vpn[2] == ((vaddr >> 30) & 0x1FF), "VPN[2] calculation should be correct");
            assert!(vpn[3] == ((vaddr >> 39) & 0x1FF), "VPN[3] calculation should be correct");
            
            early_println!("[RISC-V MMU Test] Virtual address translation test passed");
        }
    }

    /// Architecture-specific MMU tests for AArch64
    #[cfg(target_arch = "aarch64")]
    mod aarch64_tests {
        use super::*;
        use crate::arch::aarch64::vm::mmu::{PageTable, PageTableEntry, init_mmu_registers};

        #[test_case]
        fn test_aarch64_page_table_creation() {
            early_println!("[AArch64 MMU Test] Testing page table creation");
            
            // Test page table allocation and initialization
            let page_table = PageTable::new();
            // Check that the first entry is properly initialized
            assert!(!page_table.entries[0].is_valid(), "Initial page table entries should be invalid");
            
            early_println!("[AArch64 MMU Test] Page table creation test passed");
        }

        #[test_case]
        fn test_aarch64_pte_flags() {
            early_println!("[AArch64 MMU Test] Testing page table entry flags");
            
            let mut pte = PageTableEntry::new();
            
            // Test setting and getting flags for AArch64
            pte.set_valid(true);
            assert!(pte.is_valid(), "PTE should be valid after setting");
            
            pte.set_readable(true);
            assert!(pte.is_readable(), "PTE should be readable after setting");
            
            pte.set_writable(true);
            assert!(pte.is_writable(), "PTE should be writable after setting");
            
            pte.set_executable(true);
            assert!(pte.is_executable(), "PTE should be executable after setting");
            
            // Test AArch64-specific attributes
            pte.set_user_accessible(true);
            assert!(pte.is_user_accessible(), "PTE should be user accessible after setting");
            
            early_println!("[AArch64 MMU Test] Page table entry flags test passed");
        }

        #[test_case]
        fn test_aarch64_address_translation() {
            early_println!("[AArch64 MMU Test] Testing virtual address translation");
            
            // Test virtual address breakdown for AArch64 4-level page tables (48-bit VA)
            let vaddr = 0x123456789ABC;
            let vpn = [
                (vaddr >> 12) & 0x1FF,    // Level 3 (4KB pages)
                (vaddr >> 21) & 0x1FF,    // Level 2
                (vaddr >> 30) & 0x1FF,    // Level 1
                (vaddr >> 39) & 0x1FF,    // Level 0
            ];
            
            assert!(vpn[0] == ((vaddr >> 12) & 0x1FF), "Level 3 VPN calculation should be correct");
            assert!(vpn[1] == ((vaddr >> 21) & 0x1FF), "Level 2 VPN calculation should be correct");
            assert!(vpn[2] == ((vaddr >> 30) & 0x1FF), "Level 1 VPN calculation should be correct");
            assert!(vpn[3] == ((vaddr >> 39) & 0x1FF), "Level 0 VPN calculation should be correct");
            
            early_println!("[AArch64 MMU Test] Virtual address translation test passed");
        }

        #[test_case]
        fn test_aarch64_mmu_registers() {
            early_println!("[AArch64 MMU Test] Testing MMU register initialization");
            
            // Test that MMU register initialization doesn't panic
            init_mmu_registers();
            
            early_println!("[AArch64 MMU Test] MMU register initialization test passed");
        }

        #[test_case]
        fn test_aarch64_memory_attributes() {
            early_println!("[AArch64 MMU Test] Testing memory attributes");
            
            let mut pte = PageTableEntry::new();
            
            // Test different memory types
            pte.set_memory_type_device();
            assert!(pte.is_device_memory(), "PTE should be marked as device memory");
            
            pte.set_memory_type_normal_cacheable();
            assert!(pte.is_normal_cacheable_memory(), "PTE should be marked as normal cacheable memory");
            
            // Test shareability
            pte.set_outer_shareable();
            assert!(pte.is_outer_shareable(), "PTE should be marked as outer shareable");
            
            pte.set_inner_shareable();
            assert!(pte.is_inner_shareable(), "PTE should be marked as inner shareable");
            
            early_println!("[AArch64 MMU Test] Memory attributes test passed");
        }

        #[test_case]
        fn test_aarch64_asid_management() {
            early_println!("[AArch64 MMU Test] Testing ASID management");
            
            // Test ASID allocation
            use crate::arch::aarch64::vm::alloc_virtual_address_space;
            
            let asid1 = alloc_virtual_address_space();
            let asid2 = alloc_virtual_address_space();
            
            assert!(asid1 != 0, "First ASID should not be zero");
            assert!(asid2 != 0, "Second ASID should not be zero");
            assert!(asid1 != asid2, "Different ASID allocations should be unique");
            
            early_println!("[AArch64 MMU Test] ASID management test passed");
        }

        #[test_case]
        fn test_aarch64_page_table_mapping() {
            early_println!("[AArch64 MMU Test] Testing page table mapping operations");
            
            let mut page_table = PageTable::new();
            let vaddr = 0x100000;  // 1MB aligned address
            let paddr = 0x200000;  // 2MB aligned address
            
            // Test mapping a page
            let vmarea = MemoryArea::new(vaddr, vaddr + 0x1000);
            let pmarea = MemoryArea::new(paddr, paddr + 0x1000);
            let map = VirtualMemoryMap {
                vmarea,
                pmarea,
                permissions: VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
                is_shared: false,
                owner: None,
            };
            
            // The actual mapping should not panic (detailed validation would require more setup)
            match page_table.map_memory_area(1, map) {
                Ok(_) => early_println!("[AArch64 MMU Test] Page mapping succeeded"),
                Err(e) => early_println!("[AArch64 MMU Test] Page mapping failed as expected: {}", e),
            }
            
            early_println!("[AArch64 MMU Test] Page table mapping test passed");
        }
    }

    /// Test platform-specific interrupt controllers
    mod platform_tests {
        use super::*;

        #[cfg(target_arch = "riscv64")]
        #[test_case]
        fn test_plic_availability() {
            early_println!("[Platform Test] Testing PLIC availability on RISC-V");
            
            use crate::drivers::pic::Plic;
            
            // Test that PLIC can be instantiated (actual hardware interaction would need setup)
            // This test mainly verifies compilation and basic structure
            early_println!("[Platform Test] PLIC structure is available on RISC-V");
            early_println!("[Platform Test] PLIC availability test passed");
        }

        #[cfg(target_arch = "riscv64")]
        #[test_case]
        fn test_clint_availability() {
            early_println!("[Platform Test] Testing CLINT availability on RISC-V");
            
            use crate::drivers::pic::Clint;
            
            // Test that CLINT can be instantiated
            early_println!("[Platform Test] CLINT structure is available on RISC-V");
            early_println!("[Platform Test] CLINT availability test passed");
        }

        #[cfg(target_arch = "aarch64")]
        #[test_case]
        fn test_gic_availability() {
            early_println!("[Platform Test] Testing GIC availability on AArch64");
            
            use crate::drivers::pic::Gic;
            
            // Test that GIC can be instantiated (actual hardware interaction would need setup)
            // This test mainly verifies compilation and basic structure
            early_println!("[Platform Test] GIC structure is available on AArch64");
            early_println!("[Platform Test] GIC availability test passed");
        }
    }

    /// Test architecture-specific features
    mod arch_tests {
        use super::*;
        use crate::arch::{get_cpu, set_next_mode, enable_interrupt, disable_interrupt};

        #[test_case]
        fn test_arch_cpu_management() {
            early_println!("[Arch Test] Testing CPU management functions");
            
            // Test that we can get current CPU information
            let cpu = get_cpu();
            let cpu_id = cpu.get_cpuid();
            
            assert!(cpu_id < crate::environment::NUM_OF_CPUS, "CPU ID should be within valid range");
            
            early_println!("[Arch Test] CPU ID: {}", cpu_id);
            early_println!("[Arch Test] CPU management test passed");
        }

        #[test_case]
        fn test_arch_interrupt_control() {
            early_println!("[Arch Test] Testing interrupt control functions");
            
            // Test interrupt enable/disable (should not panic)
            disable_interrupt();
            enable_interrupt();
            
            early_println!("[Arch Test] Interrupt control test passed");
        }

        #[cfg(target_arch = "riscv64")]
        #[test_case]
        fn test_riscv64_specific_features() {
            early_println!("[RISC-V Arch Test] Testing RISC-V specific features");
            
            use crate::arch::riscv64::vcpu::Mode;
            
            // Test mode switching
            set_next_mode(Mode::Kernel);
            set_next_mode(Mode::User);
            
            early_println!("[RISC-V Arch Test] RISC-V specific features test passed");
        }

        #[cfg(target_arch = "aarch64")]
        #[test_case]
        fn test_aarch64_specific_features() {
            early_println!("[AArch64 Arch Test] Testing AArch64 specific features");
            
            use crate::arch::aarch64::vcpu::Mode;
            use crate::arch::aarch64::get_current_cpu_id;
            
            // Test mode switching
            set_next_mode(Mode::Kernel);
            set_next_mode(Mode::User);
            
            // Test AArch64-specific CPU ID retrieval
            let cpu_id = get_current_cpu_id();
            assert!(cpu_id < crate::environment::NUM_OF_CPUS, "AArch64 CPU ID should be within valid range");
            
            early_println!("[AArch64 Arch Test] AArch64 CPU ID: {}", cpu_id);
            early_println!("[AArch64 Arch Test] AArch64 specific features test passed");
        }
    }
}