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
//!   - The `FileSystem` trait provides unified filesystem operations for VFS v2 integration
//!
//! ## Virtual File System v2
//!
//! Scarlet implements an advanced Virtual File System (VFS v2) layer providing high-performance
//! unified file operations with container support and POSIX compatibility:
//!
//! ### Core Architecture
//!
//! - **Type-Safe File System Interface**: Modern API design with compile-time safety:
//!   - `FileSystem` trait providing standardized operations across all filesystem types
//!   - Type-safe metadata operations with `Metadata` structure for file attributes
//!   - Generic `Result<T, VfsError>` error handling with detailed error classification
//!   - Zero-copy operations where possible reducing memory allocation overhead
//!
//! - **Unified VFS Manager**: Single global filesystem namespace with isolation capabilities:
//!   - Global `VfsManager` providing unified access to all mounted filesystems
//!   - Per-process mount namespace support for container isolation
//!   - Thread-safe concurrent operations via fine-grained RwLock protection
//!   - O(1) path cache lookup improving repeated access performance
//!
//! - **Hierarchical Mount Tree**: Advanced mount management with B-tree optimization:
//!   - `MountTree` implementing efficient O(log n) mount point resolution
//!   - Support for nested mounts and complex mount hierarchies
//!   - Automatic mount point validation preventing invalid mount operations
//!   - Dynamic mount/unmount operations with consistency guarantees
//!
//! ### FileSystem Driver Architecture
//!
//! Modular driver system supporting diverse storage backends and virtual filesystems:
//!
//! - **Driver Registration System**: Dynamic filesystem driver management:
//!   - `DriverManager` singleton for runtime driver registration and discovery
//!   - Type-safe driver parameters replacing legacy string-based configuration
//!   - Support for both static (compile-time) and dynamic (runtime) driver loading
//!   - Driver versioning and compatibility checking
//!
//! - **Built-in Filesystem Drivers**:
//!   - **TmpFS**: High-performance in-memory filesystem with optional persistence
//!   - **CpioFS**: Read-only filesystem for boot archives and embedded data
//!   - **InitramFS**: Boot-time filesystem initialization with automatic extraction
//!   - **OverlayFS**: Copy-on-write layered filesystem for container images
//!
//! ### Advanced Features
//!
//! - **Path Resolution & Security**: Robust path handling with security emphasis:
//!   - Automatic path normalization preventing directory traversal attacks
//!   - Symlink resolution with loop detection and depth limits
//!   - Permission checking at each path component for security compliance
//!   - Case-sensitive and case-insensitive filesystem support
//!
//! - **File Handle Management**: Resource-safe file operations:
//!   - RAII-based automatic resource cleanup preventing file descriptor leaks
//!   - Reference-counted file objects (`Arc<dyn FileObject>`) for safe sharing
//!   - Lazy file loading reducing memory footprint for large directories
//!   - Efficient buffering strategies for optimal I/O performance
//!
//! - **System Call Integration**: Full POSIX-compatible system call support:
//!   - Direct mapping from POSIX system calls to VFS operations
//!   - Efficient `openat()`, `readdir()`, `stat()` family implementations
//!   - Advanced features like `splice()`, `sendfile()` for zero-copy operations
//!   - Support for file locks, memory mapping, and extended attributes
//!
//! ### Container & Namespace Support
//!
//! - **Mount Namespaces**: Complete filesystem isolation for containers
//! - **Bind Mounts**: Flexible directory sharing between namespaces
//! - **Read-Only Mounts**: Security-enhanced mounting with write protection
//! - **Private/Shared Mount Propagation**: Advanced mount event propagation control
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
use alloc::string::ToString;
use device::{fdt::{init_fdt, relocate_fdt, FdtManager}, manager::DeviceManager};
use environment::PAGE_SIZE;
use initcall::{call_initcalls, driver::driver_initcall_call, early::early_initcall_call};
use slab_allocator_rs::MIN_HEAP_SIZE;

use arch::{get_cpu, init_arch};
use task::{elf_loader::load_elf_into_task, new_user_task};
use vm::{kernel_vm_init, vmem::MemoryArea};
use sched::scheduler::get_scheduler;
use mem::{allocator::init_heap, init_bss, __FDT_RESERVED_START, __KERNEL_SPACE_END, __KERNEL_SPACE_START};
use timer::get_kernel_timer;
use core::panic::PanicInfo;
use crate::fs::vfs_v2::manager::init_global_vfs_manager;
use crate::fs::vfs_v2::drivers::initramfs::{init_initramfs, relocate_initramfs};


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
    /* Initialize global VFS */
    println!("[Scarlet Kernel] Initializing global VFS...");
    let manager = init_global_vfs_manager();
    /* Initialize initramfs */
    println!("[Scarlet Kernel] Initializing initramfs...");
    init_initramfs(&manager);
    /* Make init task */
    println!("[Scarlet Kernel] Creating initial user task...");
    let mut task = new_user_task("init".to_string(), 0);

    task.init();
    task.vfs = Some(manager.clone());
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
