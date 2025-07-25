//! # Scarlet Kernel
//!
//! Scarlet is an operating system kernel written in Rust that implements a transparent ABI 
//! conversion layer for executing binaries across different operating systems and architectures. 
//! The kernel provides a universal container runtime environment with strong isolation capabilities 
//! and comprehensive filesystem support.
//!
//! ## Multi-ABI Execution System
//!
//! The core innovation of Scarlet is its ability to run binaries from different operating systems
//! transparently within the same runtime environment:
//!
//! ### ABI Module Architecture
//!
//! - **Modular ABI Implementation**: Each ABI module implements its own complete syscall interface
//!   using shared kernel APIs, rather than translating between syscalls
//! - **Binary Detection**: Automatic identification of binary format and target ABI through
//!   ELF header analysis and magic number detection
//! - **Shared Kernel Resources**: All ABIs operate on common kernel objects (VFS, memory, devices)
//!   ensuring consistent behavior and efficient resource utilization
//! - **Native Implementation**: Each ABI provides full syscall implementation using underlying
//!   kernel abstractions, enabling complete OS compatibility
//!
//! ### Supported ABIs
//!
//! - **Scarlet Native ABI**: Direct kernel interface with optimal performance, featuring:
//!   - Handle-based resource management with capability-based security
//!   - Modern VFS operations with namespace isolation
//!   - Advanced IPC mechanisms including pipes and shared memory
//!   - Container-native filesystem operations
//!
//! - **Linux Compatibility ABI** *(in development)*: Full POSIX syscall implementation
//! - **xv6 Compatibility ABI** *(in development)*: Educational OS syscall implementation
//!
//! ## Container Runtime Environment
//!
//! Scarlet provides enterprise-grade containerization features:
//!
//! ### Filesystem Isolation
//!
//! - **Mount Namespace Isolation**: Per-task filesystem namespaces enabling complete isolation
//! - **Bind Mount Operations**: Selective resource sharing between containers
//! - **Overlay Filesystem**: Copy-on-write semantics with whiteout support for efficient layering
//! - **Device File Management**: Controlled access to hardware through DevFS integration
//!
//! ### Resource Management
//!
//! - **Handle-Based Security**: Capability-based access control with fine-grained permissions
//! - **Memory Isolation**: Per-task memory spaces with controlled sharing mechanisms
//! - **Task Lifecycle Management**: Complete process management with environment variable support
//! - **IPC Mechanisms**: Pipes, shared memory, and other inter-process communication primitives
//!
//! ## Virtual File System v2
//!
//! Scarlet implements a modern VFS architecture designed for container environments:
//!
//! ### Core Architecture
//!
//! - **VfsEntry**: Path hierarchy cache providing fast O(1) path resolution with automatic cleanup
//! - **VfsNode**: Abstract file entity interface with metadata access and clean downcasting
//! - **FileSystemOperations**: Unified driver API consolidating all filesystem operations
//! - **Mount Tree Management**: Hierarchical mount point management with O(log n) resolution
//!
//! ### Filesystem Drivers
//!
//! - **TmpFS**: High-performance memory-based filesystem with configurable size limits
//! - **CpioFS**: Read-only CPIO archive filesystem optimized for initramfs and embedded data
//! - **OverlayFS**: Advanced union filesystem with copy-up semantics and whiteout support
//! - **DevFS**: Device file system providing controlled hardware access
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
//! ## Boot Process
//!
//! Scarlet follows a structured initialization sequence:
//!
//! 1. **Early Architecture Init**: CPU feature detection, interrupt vector setup
//! 2. **FDT Parsing**: Hardware discovery from Flattened Device Tree
//! 3. **Memory Subsystem**: Heap allocator initialization, virtual memory setup
//! 4. **Device Discovery**: Platform device enumeration and driver binding
//! 5. **Interrupt Setup**: CLINT/PLIC initialization for timer and external interrupts
//! 6. **VFS Initialization**: Mount root filesystem, initialize global VFS manager
//! 7. **Task System**: Scheduler setup, initial task creation
//! 8. **User Space Transition**: Load initial programs and switch to user mode
//!
//! Each stage validates successful completion before proceeding, with detailed logging
//! available through the early console interface.
//!
//! ## System Integration
//!
//! ### Core Subsystems
//!
//! - **Task Management**: Complete process lifecycle with environment variables and IPC
//! - **Memory Management**: Virtual memory with per-task address spaces and shared regions
//! - **Device Framework**: Unified device interface supporting block, character, and platform devices
//! - **Interrupt Handling**: Event-driven architecture with proper context switching
//! - **Handle System**: Capability-based resource access with fine-grained permissions
//!
//! ### ABI Module Integration
//!
//! Each ABI module integrates with the kernel through standardized interfaces:
//!
//! - **Binary Loading**: ELF loader with format detection and validation
//! - **Syscall Dispatch**: Per-ABI syscall tables with transparent routing
//! - **Resource Management**: Shared kernel object access through common APIs
//! - **Environment Setup**: ABI-specific process initialization and cleanup
//! - **Mount Operations**: `mount()`, `umount()`, `pivot_root()` for dynamic filesystem management
//! - **Process Management**: `execve()`, `fork()`, `wait()`, `exit()` with proper cleanup
//! - **IPC Operations**: Pipe creation, communication, and resource sharing
//!
//! ## Architecture Support
//!
//! Currently implemented for RISC-V 64-bit architecture with comprehensive hardware support:
//!
//! - **Interrupt Handling**: Complete trap frame management with timer and external interrupts
//! - **Memory Management**: Virtual memory with page tables and memory protection
//! - **SBI Interface**: Supervisor Binary Interface for firmware communication
//! - **Instruction Abstractions**: RISC-V specific optimizations with compressed instruction support
//!
//! ## Rust Language Features
//!
//! Scarlet leverages Rust's advanced features for safe and efficient kernel development:
//!
//! ### Memory Safety
//!
//! - **Zero-cost Abstractions**: High-level constructs compile to efficient machine code
//! - **Ownership System**: Automatic memory management without garbage collection overhead
//! - **Lifetime Validation**: Compile-time prevention of use-after-free and dangling pointer errors
//! - **Borrowing Rules**: Exclusive mutable access prevents data races at compile time
//! - **No Buffer Overflows**: Array bounds checking and safe pointer arithmetic
//!
//! ### Type System Features
//!
//! - **Trait-based Design**: Generic programming with zero-cost abstractions for device drivers
//! - **Pattern Matching**: Exhaustive matching prevents unhandled error cases
//! - **Option/Result Types**: Explicit error handling without exceptions or null pointer errors
//! - **Custom Test Framework**: `#[test_case]` attribute for no-std kernel testing
//! - **Const Generics**: Compile-time array sizing and type-level programming
//!
//! ### No-std Environment
//!
//! - **Embedded-first Design**: No standard library dependency for minimal kernel footprint
//! - **Custom Allocators**: Direct control over memory allocation strategies
//! - **Inline Assembly**: Direct hardware access when needed with type safety
//! - **Custom Panic Handler**: Controlled kernel panic behavior for debugging
//! - **Boot-time Initialization**: Static initialization and controlled startup sequence
//!
//! ## Development Framework
//!
//! ### Testing Infrastructure
//!
//! Scarlet provides a comprehensive testing framework designed for kernel development:
//!
//! ```rust
//! #[test_case]
//! fn test_vfs_operations() {
//!     // Kernel unit tests run in privileged mode
//!     let vfs = VfsManager::new();
//!     // ... test implementation
//! }
//! ```
//!
//! - **Custom Test Runner**: `#[test_case]` attribute for kernel-specific testing
//! - **No-std Testing**: Tests run directly in kernel mode without standard library
//! - **Integration Tests**: Full subsystem testing including multi-ABI scenarios
//! - **Hardware-in-the-Loop**: Testing on real hardware and QEMU emulation
//! - **Performance Benchmarks**: Kernel performance measurement and regression testing
//!
//! ### Debugging Support
//!
//! - **Early Console**: Serial output available from early boot stages
//! - **Panic Handler**: Detailed panic information with stack traces
//! - **GDB Integration**: Full debugging support through QEMU's GDB stub
//! - **Memory Debugging**: Allocation tracking and leak detection
//! - **Tracing**: Event tracing for performance analysis and debugging
//!
//! ### Build System Integration
//!
//! The kernel integrates with `cargo-make` for streamlined development:
//!
//! - `cargo make build`: Full kernel build with user programs
//! - `cargo make test`: Run all kernel tests
//! - `cargo make debug`: Launch kernel with GDB support
//! - `cargo make run`: Quick development cycle execution
//!
//! ## Entry Points
//!
//! The kernel provides multiple entry points for different scenarios:
//!
//! - **`start_kernel()`**: Main bootstrap processor initialization
//! - **`start_ap()`**: Application processor startup for multicore systems
//! - **`test_main()`**: Test framework entry point when built with testing enabled
//!
//! ## Module Organization
//!
//! Core kernel modules provide focused functionality:
//!
//! - **`abi/`**: Multi-ABI implementation modules (Scarlet Native, Linux, xv6)
//! - **`arch/`**: Architecture-specific code (currently RISC-V 64-bit)
//! - **`drivers/`**: Hardware device drivers (UART, block devices, VirtIO)
//! - **`fs/`**: Filesystem implementations and VFS v2 core
//! - **`task/`**: Task management, scheduling, and process lifecycle
//! - **`mem/`**: Memory management, allocators, and virtual memory
//! - **`syscall/`**: System call dispatch and implementation
//! - **`object/`**: Kernel object system with handle management
//! - **`interrupt/`**: Interrupt handling and controller support
//!
//! *Note: Currently, Scarlet Native ABI is fully implemented. Linux and xv6 ABI support 
//! are under development and will be available in future releases.*

#![no_std]
#![no_main]
#![feature(used_with_arg)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test::test_runner)]
#![reexport_test_harness_main = "test_main"]

pub mod abi;
pub mod arch;
pub mod drivers;
pub mod interrupt;
pub mod timer;
pub mod time;
pub mod library;
pub mod mem;
pub mod traits;
pub mod sched;
pub mod sync;
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
use core::{panic::PanicInfo, sync::atomic::{fence, Ordering}};
use crate::{device::graphics::manager::GraphicsManager, fs::vfs_v2::manager::init_global_vfs_manager, interrupt::InterruptManager};
use crate::fs::vfs_v2::drivers::initramfs::{init_initramfs, relocate_initramfs};


/// A panic handler is required in Rust, this is probably the most basic one possible
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use arch::instruction::idle;

    crate::early_println!("[Scarlet Kernel] panic: {}", info);
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

    fence(Ordering::SeqCst);
    early_println!("[Scarlet Kernel] Heap initialized at {:#x} - {:#x}", heap_start, heap_end);
    
    {
        let test_vec = alloc::vec::Vec::<u8>::with_capacity(1024);
        drop(test_vec);
        early_println!("[Scarlet Kernel] Heap allocation test passed");
    }
    
    fence(Ordering::Release);

    /* After this point, we can use the heap */
    early_initcall_call();
    fence(Ordering::SeqCst); // Ensure early initcalls are completed before proceeding
    driver_initcall_call();

    #[cfg(test)]
    test_main();

    early_println!("[Scarlet Kernel] Initializing Virtual Memory...");
    let kernel_start =  unsafe { &__KERNEL_SPACE_START as *const usize as usize };
    kernel_vm_init(MemoryArea::new(kernel_start, usable_area.end));
    /* After this point, we can use the heap and virtual memory */
    /* We will also be restricted to the kernel address space */

    /* Initialize (populate) devices */
    early_println!("[Scarlet Kernel] Initializing devices...");
    DeviceManager::get_mut_manager().populate_devices();
    /* After this point, we can use the device manager */
    /* Serial console also works */
    
    /* Initialize Graphics Manager and discover graphics devices */
    early_println!("[Scarlet Kernel] Initializing graphics subsystem...");
    GraphicsManager::get_mut_manager().discover_graphics_devices();
    
    /* Initcalls */
    call_initcalls();

    /* Initialize interrupt management system */
    println!("[Scarlet Kernel] Initializing interrupt system...");
    InterruptManager::get_manager().init();

    /* Initialize timer */
    println!("[Scarlet Kernel] Initializing timer...");
    get_kernel_timer().init();

    /* Initialize scheduler */
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
    task.vfs.as_ref().unwrap().set_cwd_by_path("/").expect("Failed to set initial working directory");
    let file_obj = match task.vfs.as_ref().unwrap().open("/system/scarlet/bin/init", 0) {
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
            for map in task.vm_manager.memmap_iter() {
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
