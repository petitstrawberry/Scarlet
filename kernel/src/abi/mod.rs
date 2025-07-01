//! ABI module.
//! 
//! This module provides the interface for ABI (Application Binary Interface) modules
//! in the Scarlet kernel. ABI modules are responsible for handling system calls
//! and providing the necessary functionality for different application binary
//! interfaces.
//! 

use crate::{arch::Trapframe, fs::{drivers::overlayfs::OverlayFS, VfsManager}, task::mytask};
use alloc::{boxed::Box, string::{String, ToString}, sync::Arc};
use hashbrown::HashMap;
use spin::Mutex;

pub mod scarlet;

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
pub trait AbiModule: 'static {
    fn name() -> &'static str
    where
        Self: Sized;

    fn get_name(&self) -> String;

    /// Clone this ABI module into a boxed trait object
    /// 
    /// This method enables cloning ABI modules as trait objects,
    /// allowing each task to have its own independent ABI instance.
    fn clone_boxed(&self) -> Box<dyn AbiModule>;

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
    fn can_execute_binary(&self, _file_object: &crate::object::KernelObject, _file_path: &str, _current_abi: Option<&dyn AbiModule>) -> Option<u8> {
        // Default implementation: cannot determine
        None
    }
    
    /// Handle conversion when switching ABIs
    fn initialize_from_existing_handles(&self, _task: &crate::task::Task) -> Result<(), &'static str> {
        Ok(()) // Default: no conversion needed
    }
    
    /// Binary execution (each ABI supports its own binary format)
    /// 
    /// This method actually executes a binary that has already been verified
    /// by can_execute_binary. Use file_object.as_file() to access FileObject,
    /// and call ABI-specific loaders (ELF, PE, etc.) to load and execute the binary.
    /// 
    /// # Arguments
    /// * `file_object` - Binary file to execute (already opened, in KernelObject format)
    /// * `argv` - Command line arguments
    /// * `envp` - Environment variables
    /// * `task` - Target task (modified by this method)
    /// * `trapframe` - Execution context (modified by this method)
    /// 
    /// # Implementation Notes
    /// - Use file_object.as_file() to get FileObject
    /// - Use ABI-specific loaders (e.g., task::elf_loader)
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
}

/// ABI registry.
/// 
/// This struct is responsible for managing the registration and instantiation
/// of ABI modules in the Scarlet kernel.
/// 
pub struct AbiRegistry {
    factories: HashMap<String, fn() -> Box<dyn AbiModule>>,
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

    pub fn instantiate(name: &str) -> Option<Box<dyn AbiModule>> {
        let registry = Self::global().lock();
        registry.factories.get(name).map(|f| f())
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
            task.abi.as_ref().map(|abi| abi.as_ref())
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
    let task = mytask().unwrap();
    let abi = task.abi.as_mut().expect("ABI not set");
    abi.handle_syscall(trapframe)
}