//! ABI module.
//! 
//! This module provides the interface for ABI (Application Binary Interface) modules
//! in the Scarlet kernel. ABI modules are responsible for handling system calls
//! and providing the necessary functionality for different application binary
//! interfaces.
//! 

use crate::{arch::Trapframe, fs::VfsManager, task::mytask};
use alloc::{boxed::Box, string::{String, ToString}, sync::Arc};
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
/// # Note
/// You must implement the `Default` trait for your ABI module.
/// 
pub trait AbiModule: 'static + Send + Sync {
    fn name() -> &'static str
    where
        Self: Sized;
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
}


/// ABI registry.
/// 
/// This struct is responsible for managing the registration and instantiation
/// of ABI modules in the Scarlet kernel.
/// 
pub struct AbiRegistry {
    factories: HashMap<String, fn() -> Arc<dyn AbiModule>>,
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
            .insert(T::name().to_string(), || Arc::new(T::default()));
    }

    pub fn instantiate(name: &str) -> Option<Arc<dyn AbiModule>> {
        let registry = Self::global().lock();
        if let Some(factory) = registry.factories.get(name) {
            let abi = factory();
            return Some(abi);
        }
        None
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
    let abi = task.abi.as_ref().expect("ABI not set");
    abi.handle_syscall(trapframe)
}