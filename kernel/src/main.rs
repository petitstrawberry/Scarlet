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
//! - `cargo make build-debug-riscv64` / `cargo make build-debug-aarch64`: Full build with user programs
//! - `cargo make test-riscv64` / `cargo make test-aarch64`: Run kernel tests
//! - `cargo make debug-riscv64` / `cargo make debug-aarch64`: Launch kernel with GDB support
//! - `cargo make run-riscv64` / `cargo make run-aarch64`: Quick development cycle execution
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
pub mod boot;
pub mod device;
pub mod drivers;
pub mod earlycon;
pub mod earlyfb;
pub mod environment;
pub mod executor;
pub mod fs;
#[cfg(feature = "hypervisor")]
pub mod hypervisor;
pub mod initcall;
pub mod interrupt;
pub mod ipc;
pub mod library;
pub mod mem;
#[cfg(feature = "network")]
pub mod network;
pub mod object;
pub mod profiler;
pub mod random;
pub mod sched;
pub mod sync;
pub mod syscall;
pub mod task;
pub mod time;
pub mod timer;
pub mod traits;
pub mod vm;

#[cfg(test)]
pub mod test;

extern crate alloc;
use alloc::string::ToString;
use device::manager::{DeviceManager, DriverPriority};
use environment::PAGE_SIZE;
use initcall::{call_initcalls, driver::driver_initcall_call, early::early_initcall_call};

const MIN_HEAP_SIZE: usize = 32 * 1024;

use crate::{
    device::graphics::manager::GraphicsManager,
    executor::executor::TransparentExecutor,
    fs::{drivers::initramfs::init_initramfs, vfs_v2::manager::init_global_vfs_manager},
    interrupt::InterruptManager,
};
use arch::get_cpu;
use core::sync::atomic::{Ordering, fence};
use mem::allocator::{add_heap_region, init_heap};
use sched::scheduler::get_scheduler;
use task::new_user_task;
use timer::get_kernel_timer;
use vm::{kernel_vm_init, phys_to_virt, vmem::MemoryArea};

/// A panic handler is required in Rust, this is probably the most basic one possible
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
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
    /// Number of CPUs detected at runtime (from FDT)
    /// Used to drive SMP initialization and per-CPU resource sizing
    pub cpu_count: usize,
    pub usable_memory: MemoryArea,
    pub direct_map_area: MemoryArea,
    pub usable_memory_phys: MemoryArea,
    pub direct_map_area_phys: MemoryArea,
    pub initramfs: Option<MemoryArea>,
    pub initramfs_phys: Option<MemoryArea>,
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
    pub fn new(
        cpu_id: usize,
        cpu_count: usize,
        usable_memory: MemoryArea,
        direct_map_area: MemoryArea,
        usable_memory_phys: MemoryArea,
        direct_map_area_phys: MemoryArea,
        initramfs: Option<MemoryArea>,
        initramfs_phys: Option<MemoryArea>,
        cmdline: Option<&'static str>,
        device_source: DeviceSource,
    ) -> Self {
        Self {
            cpu_id,
            cpu_count,
            usable_memory,
            direct_map_area,
            usable_memory_phys,
            direct_map_area_phys,
            initramfs,
            initramfs_phys,
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
    let cpu_count = boot_info.cpu_count;

    early_println!("[Scarlet Kernel] Hello, I'm Scarlet kernel!");
    early_println!("[Scarlet Kernel] Boot on CPU {}", cpu_id);
    early_println!("[Scarlet Kernel] Detected {} CPU(s)", cpu_count);
    /* Use usable memory area from BootInfo */
    let usable_area = boot_info.usable_memory;
    let direct_map_area = boot_info.direct_map_area;
    let usable_area_phys = boot_info.usable_memory_phys;
    let direct_map_area_phys = boot_info.direct_map_area_phys;
    early_println!(
        "[Scarlet Kernel] Usable memory area : {:#x} - {:#x}",
        usable_area.start,
        usable_area.end
    );
    early_println!(
        "[Scarlet Kernel] Direct-map area    : {:#x} - {:#x}",
        direct_map_area.start,
        direct_map_area.end
    );

    /* Handle initramfs if available in BootInfo */
    if let Some(initramfs_area) = boot_info.initramfs {
        early_println!(
            "[Scarlet Kernel] InitramFS available: {:#x} - {:#x}",
            initramfs_area.start,
            initramfs_area.end
        );
    } else {
        early_println!("[Scarlet Kernel] No initramfs found");
    }

    early_println!("[Scarlet Kernel] Initializing PMM...");
    let pmm_start_aligned = (usable_area_phys.start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if pmm_start_aligned < usable_area_phys.end {
        unsafe {
            mem::pmm::init(MemoryArea::new(pmm_start_aligned, usable_area_phys.end));
        }
    }

    early_println!("[Scarlet Kernel] Allocating initial heap from PMM...");
    let heap_size = 512 * 1024 * 1024;
    let heap_pages = heap_size / PAGE_SIZE;
    let heap_start_phys =
        mem::pmm::alloc_contiguous_pages(heap_pages).expect("Failed to allocate heap from PMM");
    let heap_end_phys = heap_start_phys + heap_size - 1;
    let heap_start = phys_to_virt(heap_start_phys);
    let heap_end = phys_to_virt(heap_end_phys);

    early_println!("[Scarlet Kernel] Initializing heap...");
    unsafe { init_heap(heap_start, heap_size) };

    fence(Ordering::SeqCst);
    early_println!(
        "[Scarlet Kernel] Heap initialized at {:#x} - {:#x}",
        heap_start,
        heap_end
    );

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
    kernel_vm_init(
        usable_area_phys,
        direct_map_area_phys,
        boot_info.initramfs_phys,
    );
    /* After this point, we can use the heap and virtual memory */
    /* We will also be restricted to the kernel address space */

    /* Populate devices from BootInfo device source */
    early_println!("[Scarlet Kernel] Populating devices...");
    let device_manager = DeviceManager::get_manager();
    // Two-phase interrupt bring-up:
    // 1) Discover critical interrupt controllers (PLIC/CLINT) first.
    // 2) Initialize interrupt controllers.
    // 3) Discover remaining devices (which may enable specific IRQ lines).
    device_manager
        .populate_devices_from_source(&boot_info.device_source, Some(&[DriverPriority::Critical]));
    fence(Ordering::SeqCst); // Ensure device population is complete before proceeding

    /* Initialize interrupt controllers (stage 1) */
    early_println!("[Scarlet Kernel] Initializing interrupt controllers...");
    InterruptManager::get_manager().init_controllers();

    fence(Ordering::SeqCst); // Ensure interrupt controllers are initialized before proceeding

    /* Initialize NetworkManager before device discovery so protocol layers are ready */
    #[cfg(feature = "network")]
    {
        early_println!("[NetworkManager] Initializing NetworkLayers...");
        let _network_manager = crate::network::NetworkManager::init();
        fence(Ordering::SeqCst);
    }

    /* Discover remaining devices */
    early_println!("[Scarlet Kernel] Populating remaining devices...");
    device_manager.populate_devices_from_source(
        &boot_info.device_source,
        Some(&[
            DriverPriority::Core,
            DriverPriority::Standard,
            DriverPriority::Late,
        ]),
    );
    fence(Ordering::SeqCst);

    /* After this point, we can use the device manager */
    /* Serial console also works not earlyconsole, so we can use normal println! from here on */

    /* Initialize Graphics Manager and discover graphics devices */
    println!("[Scarlet Kernel] Initializing graphics subsystem...");

    // Add extra safety measures for optimized builds
    fence(Ordering::SeqCst); // Ensure device population is complete before proceeding

    // Verify that devices are actually registered before attempting graphics initialization
    let device_count = DeviceManager::get_manager().get_devices_count();
    println!(
        "[Scarlet Kernel] Found {} devices before graphics initialization",
        device_count
    );

    if device_count > 0 {
        GraphicsManager::get_manager().discover_graphics_devices();
    } else {
        println!("[Scarlet Kernel] Warning: No devices found, skipping graphics initialization");
    }

    fence(Ordering::SeqCst); // Ensure graphics devices are discovered before proceeding

    #[cfg(test)]
    test_main();

    /* Initcalls */
    println!("[boot] entering initcalls");
    call_initcalls();
    println!("[boot] leaving initcalls");

    fence(Ordering::SeqCst); // Ensure all initcalls are completed before proceeding

    /* Enable CPU interrupt reception (stage 2) */
    println!("[Scarlet Kernel] Enabling CPU interrupts...");
    InterruptManager::get_manager().enable_cpu_interrupts();

    fence(Ordering::SeqCst); // Ensure interrupt manager is initialized before proceeding

    /* Initialize timer */
    println!("[boot] Initializing timer...");
    // Initialize timer for the boot CPU (from BootInfo)
    get_kernel_timer().init(boot_info.cpu_id);

    fence(Ordering::SeqCst); // Ensure timer is initialized before proceeding

    /* Initialize scheduler */
    println!("[boot] Initializing scheduler...");
    let scheduler = get_scheduler();
    fence(Ordering::SeqCst); // Ensure scheduler is initialized before proceeding

    /* Initialize global VFS */
    println!("[boot] Initializing global VFS...");
    let manager = init_global_vfs_manager();

    /* Initialize initramfs from BootInfo if available */
    if let Some(initramfs_area) = boot_info.initramfs {
        println!("[Scarlet Kernel] Initializing initramfs from BootInfo...");
        if let Err(e) = init_initramfs(&manager, initramfs_area) {
            println!(
                "[Scarlet Kernel] Warning: Failed to initialize initramfs: {}",
                e
            );
        }
    } else {
        println!("[Scarlet Kernel] No initramfs found in BootInfo");
    }

    fence(Ordering::SeqCst); // Ensure VFS and initramfs are initialized before proceeding

    /* Apply network configuration from cmdline */
    #[cfg(feature = "network")]
    {
        let cmdline = boot_info.get_cmdline();
        if !cmdline.is_empty() {
            crate::network::config::apply_cmdline_config(cmdline);
        }
    }

    #[cfg(feature = "hypervisor")]
    {
        crate::hypervisor::init_hv();
        crate::hypervisor::init_hv_per_cpu(cpu_id);
    }

    /* Make init task */
    println!("[boot] Creating initial user task...");
    let mut task = new_user_task("init".to_string(), 0);

    task.init();
    *task.vfs.write() = Some(manager.clone());
    task.vfs
        .read()
        .as_ref()
        .unwrap()
        .set_cwd_by_path("/")
        .expect("Failed to set initial working directory");
    let init_argv = ["/system/scarlet/bin/init"];

    match TransparentExecutor::execute_binary(
        "/system/scarlet/bin/init",
        &init_argv,
        &[],
        &task,
        task.get_trapframe(),
        false,
    ) {
        Ok(()) => {
            task.vm_manager.memmaps_iter_with(|maps| {
                for map in maps {
                    println!(
                        "[Scarlet Kernel] Task memory map: {:#x} - {:#x}",
                        map.vmarea.start, map.vmarea.end
                    );
                }
            });
            println!(
                "[Scarlet Kernel] Init ELF loaded with entry point at {:#x}",
                task.vcpu.lock().get_pc()
            );
            println!("[Scarlet Kernel] Successfully loaded init ELF into task");
            println!("[Scarlet Kernel] Adding init task to scheduler...");
            let cpu_id = get_cpu().get_cpuid();
            println!("[Scarlet Kernel] cpu_id for init task: {}", cpu_id);
            get_scheduler().add_task(task, cpu_id);
            println!("[Scarlet Kernel] Init task added to scheduler");
        }
        Err(e) => println!("[Scarlet Kernel] Error loading ELF into task: {:?}", e),
    }

    println!("[Scarlet Kernel] About to fence before scheduler start...");
    fence(Ordering::SeqCst); // Ensure task is added to scheduler before proceeding
    println!("[Scarlet Kernel] Fence complete; about to print scheduler start...");

    // Use println here to avoid any potential console lock issues.
    println!("[Scarlet Kernel] Scheduler will start...");
    println!("[Scarlet Kernel] Calling start_scheduler()...");

    let next_task_id = scheduler.start_scheduler();
    if let Some(next_task_id) = next_task_id {
        let next_task = scheduler
            .get_task_by_id(next_task_id)
            .expect("First runnable task must exist");
        crate::arch::first_switch_to_user(next_task);
    }

    println!("[Scarlet Kernel] No runnable task; entering idle loop");
    loop {
        crate::arch::instruction::idle();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn start_ap(cpu_id: usize) {
    println!("[Scarlet Kernel] CPU {} is up and running", cpu_id);

    #[cfg(feature = "hypervisor")]
    {
        crate::hypervisor::init_hv_per_cpu(cpu_id);
    }

    loop {}
}
