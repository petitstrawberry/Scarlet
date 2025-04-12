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
//!   - File handles that automatically close when dropped
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
//! Scarlet implements a flexible Virtual File System (VFS) layer that provides:
//!
//! - **Filesystem Abstraction**: Common interface for multiple filesystem implementations through the `VirtualFileSystem` trait
//!   hierarchy, enabling support for various filesystems like FAT32, ext2, or custom implementations
//!
//! - **Mount Point Management**: Support for mounting filesystems at different locations with unified path handling:
//!   - Hierarchical mount points with proper path resolution
//!   - Support for mounting the same filesystem at multiple locations
//!   - Automatic mapping between absolute paths and filesystem-relative paths
//!
//! - **Path Resolution**: Normalization and resolution of file paths across different mounted filesystems:
//!   - Handling of relative paths (with `./` and `../`)
//!   - Support for absolute paths from root
//!   - Finding the most specific mount point for any given path
//!
//! - **File Operations**: Standard operations with resource safety and RAII:
//!   - Files automatically close when dropped
//!   - Buffered read/write operations
//!   - Seek operations for random file access
//!   - Directory listing and manipulation
//!
//! - **Block Device Interface**: Abstraction layer for interacting with storage devices:
//!   - Request queue for efficient I/O operations
//!   - Support for asynchronous operations
//!   - Error handling and recovery mechanisms
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
#![feature(naked_functions)]
#![feature(used_with_arg)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test::test_runner)]
#![reexport_test_harness_main = "test_main"]

pub mod arch;
pub mod drivers;
pub mod timer;
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

#[cfg(test)]
pub mod test;

extern crate alloc;
use alloc::string::ToString;
use device::{fdt::{init_fdt, relocate_fdt, FdtManager}, manager::DeviceManager};
use environment::PAGE_SIZE;
use fs::drivers::initramfs::relocate_initramfs;
use initcall::{driver::driver_initcall_call, early::early_initcall_call, initcall_task};
use slab_allocator_rs::MIN_HEAP_SIZE;

use core::panic::PanicInfo;

use arch::init_arch;
use task::new_kernel_task;
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
    relocate_initramfs(&mut usable_area).expect("Failed to relocate initramfs");
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
    
    println!("[Scarlet Kernel] Initializing timer...");
    get_kernel_timer().init();
    println!("[Scarlet Kernel] Initializing scheduler...");
    let scheduler = get_scheduler();
    /* Make idle task as initial task */
    println!("[Scarlet Kernel] Creating initial kernel task...");
    let mut task = new_kernel_task("Initcall".to_string(), 0, initcall_task);
    task.init();
    scheduler.add_task(task, cpu_id);
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
