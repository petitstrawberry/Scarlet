//! ABI module.
//! 
//! This module provides the interface for ABI (Application Binary Interface) modules
//! in the Scarlet kernel. ABI modules are responsible for handling system calls
//! and providing the necessary functionality for different application binary
//! interfaces.
//! 

use crate::{arch::Trapframe, fs::{drivers::overlayfs::OverlayFS, VfsManager}, task::mytask};
use alloc::{boxed::Box, string::{String, ToString}, sync::Arc, vec::Vec};
use hashbrown::HashMap;
use spin::Mutex;

pub mod scarlet;
pub mod xv6;

pub const MAX_ABI_LENGTH: usize = 64;

/// ABI module trait.
/// 
/// This trait defines the interface for ABI modules in the Scarlet kernel.
/// ABI modules are responsible for handling system calls and providing
/// the necessary functionality for different application binary interfaces.
/// 
/// Each ABI module must implement Clone to support task cloning with
/// independent ABI state per task.
/// 
pub trait AbiModule: Send + Sync + 'static {
    fn name() -> &'static str
    where
        Self: Sized;

    fn get_name(&self) -> String;

    /// Clone this ABI module into a boxed trait object
    /// 
    /// This method enables cloning ABI modules as trait objects,
    /// allowing each task to have its own independent ABI instance.
    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync>;

    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str>;
    
    /// Determine if a binary can be executed by this ABI and return confidence
    /// 
    /// This method reads binary content directly from the file object and
    /// executes ABI-specific detection logic (magic bytes, header structure, 
    /// entry point validation, etc.).
    /// 
    /// # Arguments
    /// * `file_object` - Binary file to check (in KernelObject format)
    /// * `file_path` - File path (for auxiliary detection like file extensions)
    /// * `current_abi` - Current task's ABI reference for inheritance/compatibility decisions
    /// 
    /// # Returns
    /// * `Some(confidence)` - Confidence level (0-100) if executable by this ABI
    /// * `None` - Not executable by this ABI
    /// 
    /// # Implementation Guidelines
    /// - Use file_object.as_file() to access FileObject
    /// - Use StreamOps::read() to directly read file content
    /// - Check ABI-specific magic bytes and header structures
    /// - Validate entry point and architecture compatibility
    /// - Consider current_abi for inheritance/compatibility bonus (same ABI = higher confidence)
    /// - Return confidence based on how well the binary matches this ABI
    /// - No need for artificial score limitations - let each ABI decide its own confidence
    /// 
    /// # Recommended Scoring Guidelines
    /// - 0-30: Basic compatibility (correct magic bytes, architecture)
    /// - 31-60: Good match (+ file extension, path hints, valid entry point)
    /// - 61-80: Strong match (+ ABI-specific headers, symbols, sections)
    /// - 81-100: Perfect match (+ same ABI inheritance, full validation)
    /// 
    /// # Example Scoring Strategy
    /// ```rust
    /// let mut confidence = 0;
    /// 
    /// // Basic format check
    /// if self.is_valid_format(file_object) { confidence += 30; }
    /// 
    /// // Entry point validation
    /// if self.is_valid_entry_point(file_object) { confidence += 15; }
    /// 
    /// // File path hints
    /// if file_path.contains(self.get_name()) { confidence += 15; }
    /// 
    /// // ABI inheritance bonus
    /// if let Some(abi) = current_abi {
    ///     if abi.get_name() == self.get_name() { confidence += 40; }
    /// }
    /// 
    /// Some(confidence.min(100))
    /// ```
    fn can_execute_binary(&self, _file_object: &crate::object::KernelObject, _file_path: &str, _current_abi: Option<&(dyn AbiModule + Send + Sync)>) -> Option<u8> {
        // Default implementation: cannot determine
        None
    }
    
    /// Handle conversion when switching ABIs
    fn initialize_from_existing_handles(&mut self, _task: &mut crate::task::Task) -> Result<(), &'static str> {
        Ok(()) // Default: no conversion needed
    }
    
    /// Convert environment variables from this ABI to Scarlet canonical format (in-place)
    /// 
    /// This method is called when switching from this ABI to another ABI.
    /// It should convert ABI-specific environment variables to a canonical
    /// Scarlet format that can then be converted to the target ABI.
    /// 
    /// Uses in-place modification to avoid expensive allocations.
    /// 
    /// # Arguments
    /// * `envp` - Mutable reference to environment variables in "KEY=VALUE" format,
    ///            will be modified to contain Scarlet canonical format
    /// 
    /// # Implementation Guidelines
    /// - Convert paths to absolute Scarlet namespace paths
    /// - Normalize variable names to Scarlet conventions
    /// - Remove ABI-specific variables that don't translate
    /// - Ensure all paths are absolute and start with /
    /// - Modify the vector in-place for efficiency
    fn normalize_env_to_scarlet(&self, _envp: &mut Vec<String>) {
        // Default: no conversion needed (assuming already in Scarlet format)
    }
    
    /// Convert environment variables from Scarlet canonical format to this ABI's format (in-place)
    /// 
    /// This method is called when switching to this ABI from another ABI.
    /// It should convert canonical Scarlet environment variables to this ABI's
    /// specific format and namespace.
    /// 
    /// Uses in-place modification to avoid expensive allocations.
    /// 
    /// # Arguments
    /// * `envp` - Mutable reference to environment variables in Scarlet canonical format,
    ///            will be modified to contain this ABI's format
    fn denormalize_env_from_scarlet(&self, _envp: &mut Vec<String>) {
        // Default: no conversion needed (assuming target is Scarlet format)
    }
    
    /// Binary execution (each ABI supports its own binary format)
    /// 
    /// This method actually executes a binary that has already been verified
    /// by can_execute_binary. Use file_object.as_file() to access FileObject,
    /// and call ABI-specific loaders (ELF, PE, etc.) to load and execute the binary.
    /// 
    /// Environment variables are passed directly as envp array, not stored in task.
    /// 
    /// # Arguments
    /// * `file_object` - Binary file to execute (already opened, in KernelObject format)
    /// * `argv` - Command line arguments
    /// * `envp` - Environment variables in "KEY=VALUE" format
    /// * `task` - Target task (modified by this method)
    /// * `trapframe` - Execution context (modified by this method)
    /// 
    /// # Implementation Notes
    /// - Use file_object.as_file() to get FileObject
    /// - Use ABI-specific loaders (e.g., task::elf_loader)
    /// - Environment variables are passed directly as envp parameter
    /// - Set task's memory space, registers, and entry point
    /// - Update trapframe registers (PC, SP) for the new process
    /// - Recommended to restore original state on execution failure
    /// 
    /// # Return Value Handling in Syscall Context
    /// The Scarlet syscall mechanism works as follows:
    /// 1. sys_execve() calls this method
    /// 2. sys_execve() returns usize to syscall_handler()
    /// 3. syscall_handler() returns Ok(usize) to syscall_dispatcher()
    /// 4. syscall_dispatcher() returns Ok(usize) to trap handler
    /// 5. Trap handler calls trapframe.set_return_value(usize) automatically
    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &mut crate::task::Task,
        trapframe: &mut Trapframe
    ) -> Result<(), &'static str>;

    /// Choose base address for ELF loading (ABI-specific strategy)
    /// 
    /// This method allows each ABI to define its own memory layout preferences
    /// for different types of ELF objects. The ELF loader will use these
    /// addresses when loading binaries for this ABI.
    /// 
    /// # Arguments
    /// * `elf_type` - ELF file type (ET_EXEC, ET_DYN, etc.)
    /// * `target` - Target component being loaded
    /// 
    /// # Returns
    /// Base address where the component should be loaded, or None to use
    /// kernel default strategy
    fn choose_load_address(&self, _elf_type: u16, _target: crate::task::elf_loader::LoadTarget) -> Option<u64> {
        None // Default: use kernel default strategy
    }
    
    /// Override interpreter path (for ABI compatibility)
    /// 
    /// This method allows each ABI to specify which dynamic linker should
    /// be used when a binary requires dynamic linking (has PT_INTERP).
    /// 
    /// # Arguments
    /// * `requested_interpreter` - Interpreter path from PT_INTERP segment
    /// 
    /// # Returns
    /// The interpreter path to actually use (may be different from requested)
    fn get_interpreter_path(&self, requested_interpreter: &str) -> String {
        requested_interpreter.to_string() // Default: use requested interpreter as-is
    }

    /// Get default working directory for this ABI
    fn get_default_cwd(&self) -> &str {
        "/" // Default: root directory
    }
    
    /// Setup overlay environment for this ABI (read-only base + writable layer)
    /// 
    /// Creates overlay filesystem with provided base VFS and paths.
    /// The TransparentExecutor is responsible for providing base_vfs, paths,
    /// and verifying that directories exist. This method assumes that required
    /// directories (/system/{abi}, /data/config/{abi}) have been prepared
    /// by the user/administrator as part of system setup.
    /// 
    /// # Arguments
    /// * `target_vfs` - VfsManager to configure with overlay filesystem
    /// * `base_vfs` - Base VFS containing system and config directories
    /// * `system_path` - Path to read-only base layer (e.g., "/system/scarlet")
    /// * `config_path` - Path to writable persistence layer (e.g., "/data/config/scarlet")
    fn setup_overlay_environment(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
        system_path: &str,
        config_path: &str,
    ) -> Result<(), &'static str> {
        // cross-vfs overlay_mount_fromはv2では未サポートのため一旦コメントアウト
        // let lower_vfs_list = alloc::vec![(base_vfs, system_path)];
        // target_vfs.overlay_mount_from(
        //     Some(base_vfs),             // upper_vfs (base VFS)
        //     config_path,                // upperdir (read-write persistent layer)
        //     lower_vfs_list,             // lowerdir (read-only base system)
        //     "/"                         // target mount point in task VFS
        // ).map_err(|e| {
        //     crate::println!("Failed to create cross-VFS overlay for ABI: {}", e.message);
        //     "Failed to create overlay environment"
        // })
        Err("overlay_mount_from (cross-vfs) is not supported in v2")
    }
    
    /// Setup shared resources accessible across all ABIs
    /// 
    /// Bind mounts common directories that should be shared from base VFS.
    /// The TransparentExecutor is responsible for providing base_vfs.
    /// 
    /// # Arguments
    /// * `target_vfs` - VfsManager to configure
    /// * `base_vfs` - Base VFS containing shared directories
    fn setup_shared_resources(
        &self,
        _target_vfs: &Arc<VfsManager>,
        _base_vfs: &Arc<VfsManager>,
    ) -> Result<(), &'static str> {
        // TODO: VFS v2 migration - update bind_mount_from API usage
        // Current limitation: function signature uses VFS v1 types
        // Bind mount shared directories from base VFS
        // target_vfs.bind_mount_from(&base_vfs, "/home", "/home")
        //     .map_err(|_| "Failed to bind mount /home")?;
        // target_vfs.bind_mount_from(&base_vfs, "/data/shared", "/data/shared")
        //     .map_err(|_| "Failed to bind mount /data/shared")?;
        // target_vfs.bind_mount_from(&base_vfs, "/", "/scarlet") // Read-onlyは未サポート
        //     .map_err(|_| "Failed to bind mount native Scarlet root to /scarlet")
        Ok(())
    }
    
    /// Handle incoming event from EventManager
    /// 
    /// This method is called when an event is delivered to a task using this ABI.
    /// Each ABI can implement its own event handling strategy:
    /// - Scarlet ABI: Handle-based queuing with EventSubscription objects
    /// - xv6 ABI: POSIX-like signals and pipe notifications
    /// - Other ABIs: Custom event processing mechanisms
    /// 
    /// # Arguments
    /// * `event` - The event to be delivered
    /// * `target_task_id` - ID of the task that should receive the event
    /// 
    /// # Returns
    /// * `Ok(())` if the event was successfully handled
    /// * `Err(message)` if event delivery failed
    fn handle_event(&self, _event: crate::ipc::Event, _target_task_id: u32) -> Result<(), &'static str> {
        // Default implementation: ignore events
        Ok(())
    }
}

/// ABI registry.
/// 
/// This struct is responsible for managing the registration and instantiation
/// of ABI modules in the Scarlet kernel.
/// 
pub struct AbiRegistry {
    factories: HashMap<String, fn() -> Box<dyn AbiModule + Send + Sync>>,
}

impl AbiRegistry {
    fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    #[allow(static_mut_refs)]
    pub fn global() -> &'static Mutex<AbiRegistry> {
        // Lazy initialization using spin lock
        static mut INSTANCE: Option<Mutex<AbiRegistry>> = None;
        static INIT: spin::Once = spin::Once::new();
        
        unsafe {
            INIT.call_once(|| {
                INSTANCE = Some(Mutex::new(AbiRegistry::new()));
            });
            
            // Safe to access after INIT.call_once is called
            INSTANCE.as_ref().unwrap()
        }
    }

    pub fn register<T>()
    where
        T: AbiModule + Default + 'static,
    {
        crate::early_println!("Registering ABI module: {}", T::name());
        let mut registry = Self::global().lock();
        registry
            .factories
            .insert(T::name().to_string(), || Box::new(T::default()));
    }

    pub fn instantiate(name: &str) -> Option<Box<dyn AbiModule + Send + Sync>> {
        let registry = Self::global().lock();
        if let Some(factory) = registry.factories.get(name) {
            let abi = factory();
            return Some(abi);
        }
        None
    }

    /// Detect the best ABI for a binary from all registered ABI modules
    /// 
    /// This method tries all registered ABIs and selects the one with the highest
    /// confidence score. Each ABI internally handles inheritance bonuses and
    /// compatibility logic based on the current task's ABI.
    /// 
    /// # Arguments
    /// * `file_object` - Binary file to check
    /// * `file_path` - File path
    /// 
    /// # Returns
    /// * `Some((abi_name, confidence))` - Best ABI name and confidence level
    /// * `None` - No executable ABI found
    pub fn detect_best_abi(file_object: &crate::object::KernelObject, file_path: &str) -> Option<(String, u8)> {
        let registry = Self::global().lock();
        
        // Get current task's ABI reference for inheritance consideration
        let current_abi = if let Some(task) = mytask() {
            Some(task.default_abi.as_ref() as &(dyn AbiModule + Send + Sync))
        } else {
            None
        };
        
        // Try all ABI modules and find the one with highest confidence
        // Each ABI decides its own confidence based on:
        // - Binary format compatibility
        // - Architecture compatibility 
        // - Entry point validity
        // - Inheritance bonus from current ABI
        registry.factories.iter()
            .filter_map(|(name, factory)| {
                let abi = factory();
                abi.can_execute_binary(file_object, file_path, current_abi)
                    .map(|confidence| (name.clone(), confidence))
            })
            .max_by_key(|(_, confidence)| *confidence)
    }
}

#[macro_export]
macro_rules! register_abi {
    ($ty:ty) => {
        crate::abi::AbiRegistry::register::<$ty>();
    };
}

pub fn syscall_dispatcher(trapframe: &mut Trapframe) -> Result<usize, &'static str> {
    // 1. Get the program counter (sepc) from trapframe
    let pc = trapframe.epc as usize;
    
    // 2. Get mutable reference to current task
    let task = mytask().unwrap();
    
    // 3. Resolve the appropriate ABI based on PC address
    let abi_module = task.resolve_abi_mut(pc);
    
    // 4. Handle the system call with the resolved ABI
    abi_module.handle_syscall(trapframe)
}