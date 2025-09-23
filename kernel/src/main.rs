//! # Scarlet Kernel
//!
//! Scarlet is an operating system kernel written in Rust that implements a transparent ABI 
//! conversion layer for executing binaries across different operating systems and architectures. 
//! The kernel provides a universal container runtime environment with strong isolation capabilities,
//! comprehensive filesystem support, dynamic linking, and modern graphics capabilities.
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
//! - **Dynamic Linking**: Native dynamic linker support for shared libraries and position-independent executables
//!
//! ### Supported ABIs
//!
//! - **Scarlet Native ABI**: Direct kernel interface with optimal performance, featuring:
//!   - Handle-based resource management with capability-based security
//!   - Modern VFS operations with namespace isolation
//!   - Advanced IPC mechanisms including pipes and event-driven communication
//!   - Container-native filesystem operations
//!   - Dynamic linking support
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
//! - **ext2**: Full ext2 filesystem implementation with complete read/write support for persistent storage
//! - **FAT32**: Complete FAT32 filesystem implementation with directory and file operations
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
//! Scarlet follows a structured, architecture-agnostic initialization sequence
//! built around the BootInfo structure for unified system startup:
//!
//! ### Architecture-Specific Boot Phase
//!
//! 1. **Low-level Initialization**: CPU feature detection, trap vector setup
//! 2. **Hardware Discovery**: Parse firmware-provided hardware description (FDT/UEFI/ACPI)
//! 3. **Memory Layout**: Determine usable memory areas and relocate critical data
//! 4. **BootInfo Creation**: Consolidate boot parameters into unified structure
//! 5. **Kernel Handoff**: Call `start_kernel()` with complete BootInfo
//!
//! ### Unified Kernel Initialization
//!
//! 6. **Early Memory Setup**: Heap allocator initialization using BootInfo memory areas
//! 7. **Early Subsystems**: Critical kernel subsystem initialization via early initcalls
//! 8. **Driver Framework**: Device driver registration and basic driver initcalls
//! 9. **Virtual Memory**: Kernel virtual memory management and address space setup
//! 10. **Device Discovery**: Hardware enumeration from BootInfo device source
//! 11. **Graphics Subsystem**: Framebuffer and graphics device initialization
//! 12. **Interrupt Infrastructure**: Interrupt controller setup and handler registration
//! 13. **Timer Subsystem**: Kernel timer initialization for scheduling and timekeeping
//! 14. **Virtual File System**: VFS initialization and root filesystem mounting
//! 15. **Initial Filesystem**: Initramfs processing if provided in BootInfo
//! 16. **Initial Process**: Create and load first userspace task (/system/scarlet/bin/init)
//! 17. **Scheduler Activation**: Begin task scheduling and enter normal operation
//!
//! ### BootInfo Integration Benefits
//!
//! - **Architecture Abstraction**: Unified interface across RISC-V, ARM, x86 platforms
//! - **Modular Design**: Clean separation between arch-specific and generic initialization
//! - **Memory Safety**: Structured memory area management prevents overlaps and corruption
//! - **Extensibility**: Easy addition of new boot parameters without breaking existing code
//! - **Debugging**: Centralized boot information for diagnostics and troubleshooting
//!
//! Each stage validates successful completion before proceeding, with comprehensive
//! logging available through the early console interface. The BootInfo structure
//! ensures all necessary information is available throughout the initialization process.
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
pub mod profiler;

#[cfg(test)]
pub mod test;

extern crate alloc;
use alloc::string::ToString;
use device::manager::DeviceManager;
use environment::PAGE_SIZE;
use initcall::{call_initcalls, driver::driver_initcall_call, early::early_initcall_call};
use slab_allocator_rs::MIN_HEAP_SIZE;

use arch::get_cpu;
use task::{elf_loader::load_elf_into_task, new_user_task};
use vm::{kernel_vm_init, vmem::MemoryArea};
use sched::scheduler::get_scheduler;
use mem::{allocator::init_heap, __KERNEL_SPACE_START};
use timer::get_kernel_timer;
use core::{panic::PanicInfo, sync::atomic::{fence, Ordering}};
use crate::{device::graphics::manager::GraphicsManager, fs::{drivers::initramfs::init_initramfs, vfs_v2::manager::init_global_vfs_manager}, interrupt::InterruptManager};

/// A panic handler is required in Rust, this is probably the most basic one possible
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use arch::instruction::idle;

    crate::early_println!("[Scarlet Kernel] panic: {}", info);

    // if let Some(task) = get_scheduler().get_current_task(get_cpu().get_cpuid()) {
    //     task.exit(1); // Exit the task with error code 1
    //     get_scheduler().schedule(get_cpu());
    // }

    loop {
        idle();
    }
}

/// Represents the source of device information during boot
/// 
/// Different boot protocols provide hardware information through various mechanisms.
/// This enum captures the source and relevant parameters for device discovery.
#[derive(Debug, Clone, Copy)]
pub enum DeviceSource {
    /// Flattened Device Tree (FDT) source with relocated FDT address
    /// Used by RISC-V, ARM, and other architectures that support device trees
    Fdt(usize),
    /// Unified Extensible Firmware Interface (UEFI) source
    /// Modern firmware interface providing comprehensive hardware information  
    Uefi,
    /// Advanced Configuration and Power Interface (ACPI) source
    /// x86/x86_64 standard for hardware configuration and power management
    Acpi,
    /// No device information available
    /// Fallback when no hardware description is provided by firmware
    None,
}

/// Boot information structure containing essential system parameters
/// 
/// This structure is created during the early boot process and contains
/// all necessary information for kernel initialization. It abstracts
/// architecture-specific boot protocols into a common interface.
/// 
/// # Architecture Integration
/// 
/// Different architectures populate this structure from their respective
/// boot protocols:
/// - **RISC-V**: Created from FDT (Flattened Device Tree) data
/// - **ARM/AArch64**: Created from FDT or UEFI
/// - **x86/x86_64**: Created from ACPI tables or legacy BIOS structures
/// 
/// # Usage
/// 
/// The BootInfo is passed to `start_kernel()` as the primary parameter
/// and provides all essential information needed for kernel initialization:
/// 
/// ```rust
/// #[no_mangle]
/// pub extern "C" fn start_kernel(boot_info: &BootInfo) -> ! {
///     // Use boot_info for system initialization
///     let memory = boot_info.usable_memory;
///     let cpu_id = boot_info.cpu_id;
///     // ...
/// }
/// ```
pub struct BootInfo {
    /// CPU/Hart ID of the boot processor
    /// Used for multicore initialization and per-CPU data structures
    pub cpu_id: usize,
    /// Usable memory area available for kernel allocation
    /// Excludes reserved regions, firmware areas, and kernel image
    pub usable_memory: MemoryArea,
    /// Optional initramfs memory area if available
    /// Contains initial root filesystem for early userspace programs
    pub initramfs: Option<MemoryArea>,
    /// Optional kernel command line parameters
    /// Boot arguments passed by bootloader for kernel configuration
    pub cmdline: Option<&'static str>,
    /// Source of device information for hardware discovery
    /// Determines how the kernel will enumerate and initialize devices
    pub device_source: DeviceSource,
}

impl BootInfo {
    /// Creates a new BootInfo instance with the specified parameters
    /// 
    /// # Arguments
    /// 
    /// * `cpu_id` - ID of the boot processor/hart
    /// * `usable_memory` - Memory area available for kernel allocation
    /// * `initramfs` - Optional initramfs memory area
    /// * `cmdline` - Optional kernel command line parameters
    /// * `device_source` - Source of device information for hardware discovery
    /// 
    /// # Returns
    /// 
    /// A new BootInfo instance containing the specified boot parameters
    pub fn new(cpu_id: usize, usable_memory: MemoryArea, initramfs: Option<MemoryArea>, cmdline: Option<&'static str>, device_source: DeviceSource) -> Self {
        Self {
            cpu_id,
            usable_memory,
            initramfs,
            cmdline,
            device_source,
        }
    }

    /// Returns the kernel command line arguments
    /// 
    /// Provides access to boot parameters passed by the bootloader.
    /// Returns an empty string if no command line was provided.
    /// 
    /// # Returns
    /// 
    /// Command line string slice, or empty string if none available
    pub fn get_cmdline(&self) -> &str {
        if let Some(cmdline) = self.cmdline {
            cmdline
        } else {
            ""
        }
    }

    /// Returns the initramfs memory area if available
    /// 
    /// The initramfs contains an initial root filesystem that can be used
    /// during early boot before mounting the real root filesystem.
    /// 
    /// # Returns
    /// 
    /// Optional memory area containing the initramfs data
    pub fn get_initramfs(&self) -> Option<MemoryArea> {
        self.initramfs
    }
}

/// Main kernel entry point for the boot processor
/// 
/// This function is called by architecture-specific boot code and performs
/// the complete kernel initialization sequence using information provided
/// in the BootInfo structure.
/// 
/// # Boot Sequence
/// 
/// The kernel initialization follows this structured sequence:
/// 
/// 1. **Early System Setup**: Extract boot parameters from BootInfo
/// 2. **Memory Initialization**: Set up heap allocator with usable memory
/// 3. **Early Initcalls**: Initialize critical early subsystems
/// 4. **Driver Initcalls**: Load and initialize device drivers
/// 5. **Virtual Memory**: Set up kernel virtual memory management
/// 6. **Device Discovery**: Enumerate hardware from BootInfo device source
/// 7. **Graphics Initialization**: Initialize graphics subsystem and framebuffer
/// 8. **Interrupt System**: Set up interrupt controllers and handlers
/// 9. **Timer Subsystem**: Initialize kernel timer and scheduling infrastructure
/// 10. **VFS Setup**: Initialize virtual filesystem and mount root
/// 11. **Initramfs Processing**: Mount initramfs if provided in BootInfo
/// 12. **Initial Task**: Create and load initial userspace process
/// 13. **Scheduler Start**: Begin task scheduling and enter normal operation
/// 
/// # Architecture Integration
/// 
/// This function is architecture-agnostic and relies on the BootInfo structure
/// to abstract hardware-specific details. Architecture-specific boot code is
/// responsible for creating a properly initialized BootInfo before calling
/// this function.
/// 
/// # Arguments
/// 
/// * `boot_info` - Comprehensive boot information structure containing:
///   - CPU ID for multicore initialization
///   - Usable memory area for heap allocation
///   - Optional initramfs location and size
///   - Kernel command line parameters
///   - Device information source (FDT/UEFI/ACPI)
/// 
/// # Memory Layout
/// 
/// The function expects the following memory layout:
/// - Kernel image loaded and executable
/// - BootInfo.usable_memory available for allocation
/// - Hardware description (FDT/ACPI) accessible via device_source
/// - Optional initramfs data at specified location
/// 
/// # Safety
/// 
/// This function assumes:
/// - Architecture-specific initialization has completed successfully
/// - BootInfo contains valid memory areas and addresses
/// - Basic CPU features (MMU, interrupts) are available
/// - Memory protection allows kernel operation
/// 
/// # Returns
/// 
/// This function never returns - it transitions to the scheduler and
/// enters normal kernel operation mode.
#[unsafe(no_mangle)]
pub extern "C" fn start_kernel(boot_info: &BootInfo) -> ! {
    let cpu_id = boot_info.cpu_id;

    early_println!("[Scarlet Kernel] Hello, I'm Scarlet kernel!");
    early_println!("[Scarlet Kernel] Boot on CPU {}", cpu_id);
    /* Use usable memory area from BootInfo */
    let usable_area = boot_info.usable_memory;
    early_println!("[Scarlet Kernel] Usable memory area : {:#x} - {:#x}", usable_area.start, usable_area.end);
    
    /* Handle initramfs if available in BootInfo */
    if let Some(initramfs_area) = boot_info.initramfs {
        early_println!("[Scarlet Kernel] InitramFS available: {:#x} - {:#x}", 
                      initramfs_area.start, initramfs_area.end);
        // Note: initramfs already relocated by arch-specific boot code
    } else {
        early_println!("[Scarlet Kernel] No initramfs found");
    }
    
    /* Initialize heap with the usable memory area */
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

    early_println!("[Scarlet Kernel] Initializing Virtual Memory...");
    let kernel_start =  unsafe { &__KERNEL_SPACE_START as *const usize as usize };
    kernel_vm_init(MemoryArea::new(kernel_start, usable_area.end));
    /* After this point, we can use the heap and virtual memory */
    /* We will also be restricted to the kernel address space */

    /* Populate devices from BootInfo device source */
    early_println!("[Scarlet Kernel] Populating devices...");
    let device_manager = DeviceManager::get_mut_manager();
    device_manager.populate_devices_from_source(&boot_info.device_source, None);
    fence(Ordering::SeqCst); // Ensure device population is complete before proceeding
    /* After this point, we can use the device manager */
    /* Serial console also works */
    
    /* Initialize Graphics Manager and discover graphics devices */
    early_println!("[Scarlet Kernel] Initializing graphics subsystem...");
    
    // Add extra safety measures for optimized builds
    fence(Ordering::SeqCst); // Ensure device population is complete before proceeding
    
    // Verify that devices are actually registered before attempting graphics initialization
    let device_count = DeviceManager::get_manager().get_devices_count();
    early_println!("[Scarlet Kernel] Found {} devices before graphics initialization", device_count);
    
    if device_count > 0 {
        GraphicsManager::get_mut_manager().discover_graphics_devices();
    } else {
        early_println!("[Scarlet Kernel] Warning: No devices found, skipping graphics initialization");
    }
    
    fence(Ordering::SeqCst); // Ensure graphics devices are discovered before proceeding

    #[cfg(test)]
    test_main();
    
    /* Initcalls */
    call_initcalls();

    fence(Ordering::SeqCst); // Ensure all initcalls are completed before proceeding

    /* Initialize interrupt management system */
    println!("[Scarlet Kernel] Initializing interrupt system...");
    InterruptManager::get_manager().init();

    fence(Ordering::SeqCst); // Ensure interrupt manager is initialized before proceeding

    /* Initialize timer */
    println!("[Scarlet Kernel] Initializing timer...");
    get_kernel_timer().init();

    fence(Ordering::SeqCst); // Ensure timer is initialized before proceeding

    /* Initialize scheduler */
    println!("[Scarlet Kernel] Initializing scheduler...");
    let scheduler = get_scheduler();
    fence(Ordering::SeqCst); // Ensure scheduler is initialized before proceeding

    /* Initialize global VFS */
    println!("[Scarlet Kernel] Initializing global VFS...");
    let manager = init_global_vfs_manager();
    
    /* Initialize initramfs from BootInfo if available */
    if let Some(initramfs_area) = boot_info.initramfs {
        println!("[Scarlet Kernel] Initializing initramfs from BootInfo...");
        if let Err(e) = init_initramfs(&manager, initramfs_area) {
            println!("[Scarlet Kernel] Warning: Failed to initialize initramfs: {}", e);
        }
    } else {
        println!("[Scarlet Kernel] No initramfs found in BootInfo");
    }

    fence(Ordering::SeqCst); // Ensure VFS and initramfs are initialized before proceeding

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

    fence(Ordering::SeqCst); // Ensure task is added to scheduler before proceeding

    println!("[Scarlet Kernel] Scheduler will start...");
    scheduler.start_scheduler();
    loop {} 
}

#[unsafe(no_mangle)]
pub extern "C" fn start_ap(cpu_id: usize) {
    println!("[Scarlet Kernel] CPU {} is up and running", cpu_id);

    loop {}
}
