//! # Scarlet Kernel
//!
//! The Scarlet Kernel is a bare metal, `no_std` operating system kernel designed with architecture 
//! flexibility in mind. It aims to provide a clean design with strong safety guarantees 
//! through Rust's ownership model.
//!
//! While the current implementation establishes fundamental kernel functionality, our long-term
//! vision is to develop a fully modular operating system where components can be dynamically
//! loaded and unloaded at runtime, similar to loadable kernel modules in other systems.
//!
//! ## Core Features
//!
//! - **No Standard Library**: Built using `#![no_std]` for bare metal environments, implementing only the essential
//!   functionality needed for kernel operation without relying on OS-specific features
//! - **Multi-Architecture Design**: Currently implemented for RISC-V 64-bit, with a clean abstraction layer designed
//!   for supporting multiple architectures in the future
//! - **Memory Management**: Custom heap allocator with virtual memory support that handles physical and virtual memory
//!   mapping, page tables, and memory protection
//! - **Task Scheduling**: Cooperative and preemptive multitasking with priority-based scheduling and support for
//!   kernel and user tasks
//! - **Driver Framework**: Organized driver architecture with device discovery through FDT (Flattened Device Tree),
//!   supporting hot-pluggable and fixed devices
//! - **Filesystem Support**: Flexible Virtual File System (VFS) layer with support for mounting multiple filesystem
//!   implementations and unified path handling
//! - **Hardware Abstraction**: Clean architecture-specific abstractions that isolate architecture-dependent code
//!   to facilitate porting to different architectures
//! - **Future Modularity**: Working toward a fully modular design with runtime-loadable kernel components
//!
//! ## Resource Management with Rust's Ownership Model
//!
//! Scarlet leverages Rust's ownership and borrowing system to provide memory safety without garbage collection:
//!
//! - **Zero-Cost Abstractions**: Using Rust's type system for resource management without runtime overhead. For example,
//!   the device driver system uses traits to define common interfaces while allowing specialized implementations
//!   with no virtual dispatch cost when statically resolvable.
//!
//! - **RAII Resource Management**: Kernel resources are automatically cleaned up when they go out of scope, including:
//!   - File objects that automatically close when dropped
//!   - Memory allocations that are properly freed
//!   - Device resources that are released when no longer needed
//!
//! - **Mutex and RwLock**: Thread-safe concurrent access to shared resources using the `spin` crate's lock implementations:
//!   - The scheduler uses locks to protect its internal state during task switching
//!   - Device drivers use locks to ensure exclusive access to hardware
//!   - Filesystem operations use RwLocks to allow concurrent reads but exclusive writes
//!
//! - **Arc** (Atomic Reference Counting): Safe sharing of resources between kernel components:
//!   - Filesystem implementations are shared between multiple mount points
//!   - Device instances can be referenced by multiple drivers
//!   - System-wide singletons are managed safely with interior mutability patterns
//!
//! - **Memory Safety**: Prevention of use-after-free, double-free, and data races at compile time:
//!   - The type system ensures resources are not used after being freed
//!   - Mutable references are exclusive, preventing data races
//!   - Lifetimes ensure references do not outlive the data they point to
//!
//! - **Trait-based Abstractions**: Common interfaces for device drivers and subsystems enabling modularity:
//!   - The `BlockDevice` trait defines operations for block-based storage
//!   - The `SerialDevice` trait provides a common interface for UART and console devices
//!   - The `FileSystem` and `FileOperations` traits allow different filesystem implementations
//!
//! ## Virtual File System
//!
//! Scarlet implements a highly flexible Virtual File System (VFS) layer designed for
//! containerization and process isolation with advanced bind mount capabilities:
//!
//! ### Core Architecture
//!
//! - **Per-Task VFS Management**: Each task can have its own isolated `VfsManager` instance:
//!   - Tasks store `Option<Arc<VfsManager>>` allowing independent filesystem namespaces
//!   - Support for complete filesystem isolation or selective resource sharing
//!   - Thread-safe operations via RwLock protection throughout the VFS layer
//!
//! - **Filesystem Driver Framework**: Modular driver system with type-safe parameter handling:
//!   - Global `FileSystemDriverManager` singleton for driver registration and management
//!   - Support for block device, memory-based, and virtual filesystem creation
//!   - Structured parameter system replacing old string-based configuration
//!   - Dynamic dispatch enabling future runtime filesystem module loading
//!
//! - **Enhanced Mount Tree**: Hierarchical mount point management with bind mount support:
//!   - O(log k) path resolution performance where k is path depth
//!   - Independent mount point namespaces per VfsManager instance
//!   - Security-enhanced path normalization preventing directory traversal attacks
//!   - Efficient Trie-based mount point storage reducing memory usage
//!
//! ### Bind Mount Functionality
//!
//! Advanced bind mount capabilities for flexible directory mapping and container orchestration:
//!
//! - **Basic Bind Mounts**: Mount directories from one location to another within the same VfsManager
//! - **Cross-VFS Bind Mounts**: Share directories between isolated VfsManager instances for container resource sharing
//! - **Read-Only Bind Mounts**: Security-enhanced mounting with write protection
//! - **Shared Bind Mounts**: Mount propagation sharing for complex namespace scenarios
//! - **Thread-Safe Operations**: Bind mount operations callable from system call context
//!
//! ### Path Resolution & Security
//!
//! - **Normalized Path Handling**: Automatic resolution of relative paths (`.` and `..`)
//! - **Security Protection**: Prevention of directory traversal attacks through path validation
//! - **Transparent Resolution**: Seamless handling of bind mounts and nested mount points
//! - **Performance Optimization**: Efficient path lookup with O(log k) complexity
//!
//! ### File Operations & Resource Management
//!
//! - **RAII Resource Safety**: Files automatically close when dropped, preventing resource leaks
//! - **Thread-Safe File Access**: Concurrent file operations with proper locking
//! - **Handle Management**: Arc-based file object sharing with automatic cleanup
//! - **Directory Operations**: Complete directory manipulation with metadata support
//!
//! ### Storage Integration
//!
//! - **Block Device Interface**: Abstraction layer for storage device interaction
//! - **Memory-Based Filesystems**: Support for RAM-based filesystems like tmpfs
//! - **Hybrid Filesystem Support**: Filesystems operating on both block devices and memory
//! - **Device File Support**: Integration with character and block device management
//!
//! ## Boot Process
//!
//! The kernel has two main entry points:
//! - `start_kernel`: Main boot entry point for the bootstrap processor
//! - `start_ap`: Entry point for application processors (APs) in multicore systems
//!
//! The initialization sequence for the bootstrap processor includes:
//! 1. `.bss` section initialization (zeroing)
//! 2. Architecture-specific initialization (setting up CPU features)
//! 3. FDT (Flattened Device Tree) parsing for hardware discovery
//! 4. Heap initialization enabling dynamic memory allocation
//! 5. Early driver initialization via the initcall mechanism
//! 6. Driver registration and initialization (serial, block devices, etc.)
//! 7. Virtual memory setup with kernel page tables
//! 8. Device discovery and initialization based on FDT data
//! 9. Timer initialization for scheduling and timeouts
//! 10. Scheduler initialization and initial task creation
//! 11. Task scheduling and transition to the kernel main loop
//!
//! ## Current Architecture Implementation
//!
//! The current RISC-V implementation includes:
//! - Boot sequence utilizing SBI (Supervisor Binary Interface) for hardware interaction
//! - Support for S-mode operation
//! - Interrupt handling through trap frames with proper context saving/restoring
//! - Memory management with Sv48 virtual memory addressing
//! - Architecture-specific timer implementation
//! - Support for multiple privilege levels
//! - Instruction abstractions for atomic operations and privileged instructions
//!
//! ## Testing Framework
//!
//! Scarlet includes a custom testing framework that allows:
//! - Unit tests for kernel components
//! - Integration tests for subsystem interaction
//! - Boot tests to verify initialization sequence
//! - Hardware-in-the-loop tests when running on real or emulated hardware
//!
//! ## Development Notes
//!
//! The kernel uses Rust's advanced features like naked functions and custom test frameworks.
//! In non-test builds, a simple panic handler is provided that prints the panic information 
//! and enters an infinite loop. The kernel makes extensive use of Rust's unsafe code where
//! necessary for hardware interaction while maintaining safety guarantees through careful
//! abstraction boundaries.

#![no_std]
#![no_main]
#![feature(used_with_arg)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test::test_runner)]
#![reexport_test_harness_main = "test_main"]

pub mod abi;
pub mod arch;
pub mod drivers;
pub mod timer;
pub mod time;
pub mod library;
pub mod mem;
pub mod traits;
pub mod sched;
pub mod earlycon;
pub mod environment;
pub mod vm;
pub mod task;
pub mod initcall;
pub mod syscall;
pub mod device;
pub mod fs;
pub mod object;
pub mod ipc;
pub mod executor;

#[cfg(test)]
pub mod test;

extern crate alloc;
use alloc::{string::ToString, sync::Arc};
use device::{fdt::{init_fdt, relocate_fdt, FdtManager}, manager::DeviceManager};
use environment::PAGE_SIZE;
use fs::{drivers::initramfs::{init_initramfs, relocate_initramfs}, VfsManager};
use initcall::{call_initcalls, driver::driver_initcall_call, early::early_initcall_call};
use slab_allocator_rs::MIN_HEAP_SIZE;

use core::panic::{self, PanicInfo};

use arch::{get_cpu, init_arch};
use task::{elf_loader::load_elf_into_task, new_user_task};
use vm::{kernel_vm_init, vmem::MemoryArea};
use sched::scheduler::get_scheduler;
use mem::{allocator::init_heap, init_bss, __FDT_RESERVED_START, __KERNEL_SPACE_END, __KERNEL_SPACE_START};
use timer::get_kernel_timer;


/// A panic handler is required in Rust, this is probably the most basic one possible
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use arch::instruction::idle;

    println!("[Scarlet Kernel] panic: {}", info);
    loop {
        idle();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn start_kernel(cpu_id: usize) -> ! {
    early_println!("Hello, I'm Scarlet kernel!");
    early_println!("[Scarlet Kernel] Boot on CPU {}", cpu_id);
    early_println!("[Scarlet Kernel] Initializing .bss section...");
    init_bss();
    early_println!("[Scarlet Kernel] Initializing arch...");
    init_arch(cpu_id);
    /* Initializing FDT subsystem */
    early_println!("[Scarlet Kernel] Initializing FDT...");
    init_fdt();
    /* Get DRAM area from FDT */
    let dram_area = FdtManager::get_manager().get_dram_memoryarea().expect("Memory area not found");
    early_println!("[Scarlet Kernel] DRAM area          : {:#x} - {:#x}", dram_area.start, dram_area.end);
    /* Relocate FDT to usable memory area */
    early_println!("[Scarlet Kernel] Relocating FDT...");
    let fdt_reloc_start = unsafe { &__FDT_RESERVED_START as *const usize as usize };
    let dest_ptr = fdt_reloc_start as *mut u8;
    relocate_fdt(dest_ptr);
    /* Calculate usable memory area */
    let kernel_end =  unsafe { &__KERNEL_SPACE_END as *const usize as usize };
    let mut usable_area = MemoryArea::new(kernel_end, dram_area.end);
    early_println!("[Scarlet Kernel] Usable memory area : {:#x} - {:#x}", usable_area.start, usable_area.end);
    /* Relocate initramfs to usable memory area */
    early_println!("[Scarlet Kernel] Relocating initramfs...");
    if let Err(e) = relocate_initramfs(&mut usable_area) {
        early_println!("[Scarlet Kernel] Failed to relocate initramfs: {}", e);
    }
    early_println!("[Scarlet Kernel] Updated Usable memory area : {:#x} - {:#x}", usable_area.start, usable_area.end);
    /* Initialize heap with the usable memory area after FDT */
    early_println!("[Scarlet Kernel] Initializing heap...");
    let heap_start = (usable_area.start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let heap_size = ((usable_area.end - heap_start + 1) / MIN_HEAP_SIZE) * MIN_HEAP_SIZE;
    let heap_end = heap_start + heap_size - 1;
    init_heap(MemoryArea::new(heap_start, heap_end));
    /* After this point, we can use the heap */
    early_initcall_call();
    driver_initcall_call();
    /* Serial console also works */

    #[cfg(test)]
    test_main();

    println!("[Scarlet Kernel] Initializing Virtual Memory...");
    let kernel_start =  unsafe { &__KERNEL_SPACE_START as *const usize as usize };
    kernel_vm_init(MemoryArea::new(kernel_start, usable_area.end));
    /* After this point, we can use the heap and virtual memory */
    /* We will also be restricted to the kernel address space */

    /* Initialize (populate) devices */
    println!("[Scarlet Kernel] Initializing devices...");
    DeviceManager::get_mut_manager().populate_devices();
    /* Initcalls */
    call_initcalls();
    /* Initialize timer */
    println!("[Scarlet Kernel] Initializing timer...");
    get_kernel_timer().init();
    println!("[Scarlet Kernel] Initializing scheduler...");
    let scheduler = get_scheduler();
    /* Initialize initramfs */
    println!("[Scarlet Kernel] Initializing initramfs...");
    let mut manager = VfsManager::new();
    init_initramfs(&mut manager);
    /* Make init task */
    println!("[Scarlet Kernel] Creating initial user task...");
    let mut task = new_user_task("init".to_string(), 0);

    task.init();
    let manager_arc = Arc::new(manager);
    task.base_vfs = Some(manager_arc.clone());
    task.vfs = Some(manager_arc);
    task.cwd = Some("/".to_string());
    let file_obj = match task.vfs.as_ref().unwrap().open("/bin/init", 0) {
        Ok(kernel_obj) => kernel_obj,
        Err(e) => {
            panic!("Failed to open init file: {:?}", e);
        },
    };
    // file_obj is already a KernelObject::File
    let file_ref = match file_obj.as_file() {
        Some(file) => file,
        None => panic!("Failed to get file reference"),
    };

    match load_elf_into_task(file_ref, &mut task) {
        Ok(_) => {
            for map in task.vm_manager.get_memmap() {
                early_println!("[Scarlet Kernel] Task memory map: {:#x} - {:#x}", map.vmarea.start, map.vmarea.end);
            }
            early_println!("[Scarlet Kernel] Successfully loaded init ELF into task");
            get_scheduler().add_task(task, get_cpu().get_cpuid());
        }
        Err(e) => early_println!("[Scarlet Kernel] Error loading ELF into task: {:?}", e),
    }

    println!("[Scarlet Kernel] Scheduler will start...");
    scheduler.start_scheduler();
    loop {} 
}

#[unsafe(no_mangle)]
pub extern "C" fn start_ap(cpu_id: usize) {
    println!("[Scarlet Kernel] CPU {} is up and running", cpu_id);
    println!("[Scarlet Kernel] Initializing arch...");
    init_arch(cpu_id);
    loop {}
}
