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
    
    /// Setup ABI-specific VFS environment
    /// 
    /// This method configures the VFS with ABI-specific directory structures
    /// and filesystem mounts. It uses the existing VfsManager bind mount
    /// functionality to create the required environment.
    /// 
    /// # Arguments
    /// * `vfs` - VfsManager to configure (should already have a root filesystem)
    fn setup_vfs_environment(&self, _vfs: &mut crate::fs::VfsManager) -> Result<(), &'static str> {
        // Default: Unix-compatible environment (no special setup needed)
        Ok(())
    }
    

    
    /// Create initial VFS for this ABI
    fn create_initial_vfs(&self) -> Result<alloc::sync::Arc<crate::fs::VfsManager>, &'static str> {
        let mut vfs = crate::fs::VfsManager::new();
        
        // Create basic root filesystem (tmpfs)
        let params = crate::fs::params::TmpFSParams::default();
        let rootfs_id = vfs.create_and_register_fs_with_params("tmpfs", &params)
            .map_err(|_| "Failed to create root filesystem")?;
        vfs.mount(rootfs_id, "/")
            .map_err(|_| "Failed to mount root filesystem")?;
        
        // Setup ABI-specific environment
        self.setup_vfs_environment(&mut vfs)?;
        
        Ok(alloc::sync::Arc::new(vfs))
    }
    
    /// Get default working directory for this ABI
    fn get_default_cwd(&self) -> &str {
        "/" // Default: root directory
    }

    /// Setup ABI-specific VFS environment
    /// 
    /// This method takes an existing VFS as a reference and creates a new VFS
    /// with ABI-specific bind mounts applied. In the future, this could be
    /// implemented as actual bind mounts, but currently creates a new VFS
    /// with the desired layout.
    /// 
    /// # Arguments
    /// * `base_vfs` - The existing VFS to use as reference
    /// 
    /// # Returns
    /// A new VfsManager with ABI-specific configuration applied
    fn setup_abi_vfs(&self, base_vfs: &alloc::sync::Arc<crate::fs::VfsManager>) -> Result<alloc::sync::Arc<crate::fs::VfsManager>, &'static str> {
        // Default implementation: clone the base VFS without modifications
        // ABIs can override this to add their specific bind mounts
        
        // Create a new VFS starting from the base VFS structure
        let mut abi_vfs = crate::fs::VfsManager::new();
        
        // Copy root filesystem from base
        // TODO: Implement VfsManager::copy_from or similar functionality
        // For now, create a basic filesystem
        let params = crate::fs::params::TmpFSParams::default();
        let rootfs_id = abi_vfs.create_and_register_fs_with_params("tmpfs", &params)
            .map_err(|_| "Failed to create ABI VFS root filesystem")?;
        abi_vfs.mount(rootfs_id, "/")
            .map_err(|_| "Failed to mount ABI VFS root filesystem")?;
        
        // Apply ABI-specific bind mounts
        self.apply_abi_bind_mounts(&mut abi_vfs, base_vfs)?;
        
        Ok(alloc::sync::Arc::new(abi_vfs))
    }
    
    /// Apply ABI-specific bind mounts to the VFS
    /// 
    /// # Arguments
    /// * `abi_vfs` - The VFS to apply bind mounts to
    /// * `base_vfs` - The base VFS to bind mount from
    fn apply_abi_bind_mounts(&self, _abi_vfs: &mut crate::fs::VfsManager, _base_vfs: &alloc::sync::Arc<crate::fs::VfsManager>) -> Result<(), &'static str> {
        // Default: no ABI-specific bind mounts
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
        $crate::abi::AbiRegistry::register::<$ty>();
    };
}

pub fn syscall_dispatcher(trapframe: &mut Trapframe) -> Result<usize, &'static str> {
    let task = mytask().unwrap();
    let abi = task.abi.as_deref_mut().expect("ABI not set");
    abi.handle_syscall(trapframe)
}