//! ABI module.
//! 
//! This module provides the interface for ABI (Application Binary Interface) modules
//! in the Scarlet kernel. ABI modules are responsible for handling system calls
//! and providing the necessary functionality for different application binary
//! interfaces.
//! 

use crate::{arch::Trapframe, task::mytask};
use alloc::{boxed::Box, string::{String, ToString}};
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
pub trait AbiModule: 'static {
    fn name() -> &'static str
    where
        Self: Sized;

    fn get_name(&self) -> String;

    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str>;
    
    /// Determine if a binary can be executed by this ABI
    /// 
    /// This method reads binary content directly from the file object and
    /// executes ABI-specific detection logic (magic bytes, header structure, etc.).
    /// 
    /// # Arguments
    /// * `file_object` - Binary file to check (in KernelObject format)
    /// * `file_path` - File path (for auxiliary detection like file extensions)
    /// 
    /// # Returns
    /// * `Some(confidence)` - Confidence level (0-100) if executable
    /// * `None` - Not executable by this ABI
    /// 
    /// # Implementation Notes
    /// - Use file_object.as_file() to access FileObject
    /// - Use StreamOps::read() to directly read file content
    /// - Check ABI-specific magic bytes or header structures
    /// - Combine with path extensions to determine confidence level
    fn can_execute_binary(&self, _file_object: &crate::object::KernelObject, _file_path: &str) -> Option<u8> {
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
    /// This creates the immutable infrastructure overlay:
    /// - `/system/{abi}` as read-only base layer from global VFS
    /// - `/data/config/{abi}` as writable persistence layer from global VFS
    /// 
    /// # Arguments
    /// * `vfs` - VfsManager to configure with overlay filesystem
    fn setup_overlay_environment(&self, vfs: &mut crate::fs::VfsManager) -> Result<(), &'static str> {
        let abi_name = self.get_name();
        let system_path = alloc::format!("/system/{}", abi_name);
        let config_path = alloc::format!("/data/config/{}", abi_name);
        let global_vfs = crate::fs::get_global_vfs();
        
        // Ensure ABI directories exist in global VFS
        global_vfs.create_dir(&system_path).ok();
        global_vfs.create_dir(&config_path).ok();
        
        // Create cross-VFS overlay mount
        let lower_vfs_list = alloc::vec![(global_vfs, system_path.as_str())];
        vfs.overlay_mount_from(
            Some(global_vfs),           // upper_vfs (global VFS)
            &config_path,               // upperdir (read-write persistent layer)
            lower_vfs_list,             // lowerdir (read-only base system)
            "/"                         // target mount point in task VFS
        ).map_err(|e| {
            crate::early_println!("Failed to create cross-VFS overlay for ABI {}: {}", abi_name, e.message);
            "Failed to create overlay environment"
        })
    }
    
    /// Setup shared resources accessible across all ABIs
    /// 
    /// This bind mounts common directories that should be shared:
    /// - `/home` - User home directories
    /// - `/data/shared` - Shared application data
    /// - `/scarlet` - Official gateway to native Scarlet environment (read-only)
    /// 
    /// # Arguments
    /// * `vfs` - VfsManager to configure
    fn setup_shared_resources(&self, vfs: &mut crate::fs::VfsManager) -> Result<(), &'static str> {
        let global_vfs = crate::fs::get_global_vfs();
        
        // Bind mount shared directories
        vfs.bind_mount_from(global_vfs, "/home", "/home", false)
            .map_err(|_| "Failed to bind mount /home")?;
        
        vfs.bind_mount_from(global_vfs, "/data/shared", "/data/shared", false)
            .map_err(|_| "Failed to bind mount /data/shared")?;
        
        // Setup official gateway to native Scarlet environment
        vfs.bind_mount_from(global_vfs, "/", "/scarlet", true) // Read-only for security
            .map_err(|_| "Failed to bind mount native Scarlet root to /scarlet")
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
    /// # Arguments
    /// * `file_object` - Binary file to check
    /// * `file_path` - File path
    /// 
    /// # Returns
    /// * `Some((abi_name, confidence))` - Best ABI name and confidence level
    /// * `None` - No executable ABI found
    pub fn detect_best_abi(file_object: &crate::object::KernelObject, file_path: &str) -> Option<(String, u8)> {
        let registry = Self::global().lock();
        let mut best_match: Option<(String, u8)> = None;
        
        // Try all ABI modules and select the one with highest confidence
        for (name, factory) in &registry.factories {
            let abi = factory();
            if let Some(confidence) = abi.can_execute_binary(file_object, file_path) {
                match &best_match {
                    None => best_match = Some((name.clone(), confidence)),
                    Some((_, best_confidence)) => {
                        if confidence > *best_confidence {
                            best_match = Some((name.clone(), confidence));
                        }
                    }
                }
            }
        }
        
        best_match
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
    let abi = task.abi.as_deref_mut().expect("ABI not set");
    abi.handle_syscall(trapframe)
}