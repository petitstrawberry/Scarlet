//! TransparentExecutor Implementation
//!
//! The TransparentExecutor provides unified exec API for all ABIs.
//! It does NOT contain ABI-specific knowledge - each ABI module handles
//! its own binary format and conversion logic.

use crate::{fs::manager::get_global_vfs_manager, task::Task};
use crate::arch::Trapframe;
use crate::vm::vmem::VirtualMemoryMap;
use crate::task::ManagedPage;
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, sync::Arc};
use core::fmt;

/// Task state backup for exec rollback
/// 
/// This structure contains a complete backup of task state that can be
/// restored if execve fails. Includes memory state, metadata, and trapframe.
#[derive(Debug)]
struct TaskStateBackup {
    managed_pages: Vec<ManagedPage>,
    vm_mapping: Vec<VirtualMemoryMap>,
    text_size: usize,
    data_size: usize,
    stack_size: usize,
    name: String,
    trapframe: Trapframe,
}

impl TaskStateBackup {
    /// Create a backup of the current task state including trapframe
    /// 
    /// This creates a complete snapshot that can be restored if exec fails.
    fn create_backup(task: &mut Task, trapframe: &Trapframe) -> Self {
        // Move managed pages to backup (avoiding clone)
        let mut backup_pages = Vec::new();
        backup_pages.append(&mut task.managed_pages);
        
        // Backup VM mapping
        let backup_vm_mapping = task.vm_manager.remove_all_memory_maps();
        
        Self {
            managed_pages: backup_pages,
            vm_mapping: backup_vm_mapping,
            text_size: task.text_size,
            data_size: task.data_size,
            stack_size: task.stack_size,
            name: task.name.clone(),
            trapframe: trapframe.clone(),
        }
    }
    
    /// Restore task state from backup including trapframe
    /// 
    /// This restores the complete task state from a previous backup,
    /// ensuring full rollback on exec failure.
    fn restore_to_task(self, task: &mut Task, trapframe: &mut Trapframe) -> Result<(), &'static str> {
        // Restore managed pages
        task.managed_pages = self.managed_pages;
        
        // Restore VM mapping
        task.vm_manager.restore_memory_maps(self.vm_mapping)?;
        
        // Restore sizes and name
        task.text_size = self.text_size;
        task.data_size = self.data_size;
        task.stack_size = self.stack_size;
        task.name = self.name;
        
        // Restore trapframe
        *trapframe = self.trapframe;
        
        Ok(())
    }
}

/// Errors that can occur during transparent execution
#[derive(Debug, Clone)]
pub enum ExecutorError {
    /// Binary format not recognized
    UnknownBinaryFormat,
    /// ABI not found or not supported
    UnsupportedAbi(String),
    /// Execution failed
    ExecutionFailed(String),
    /// Resource allocation failed
    ResourceAllocationFailed,
}

impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutorError::UnknownBinaryFormat => write!(f, "Unknown binary format"),
            ExecutorError::UnsupportedAbi(abi) => write!(f, "Unsupported ABI: {}", abi),
            ExecutorError::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
            ExecutorError::ResourceAllocationFailed => write!(f, "Resource allocation failed"),
        }
    }
}

/// Result type for executor operations
pub type ExecutorResult<T> = Result<T, ExecutorError>;

/// TransparentExecutor provides unified exec API
/// 
/// This executor:
/// - Analyzes binary format and detects appropriate ABI
/// - Delegates execution to the detected ABI module
/// - Does NOT contain ABI-specific conversion logic
/// - Provides VFS inheritance and resource management
pub struct TransparentExecutor;

impl TransparentExecutor {
    /// Execute a binary with automatic ABI detection
    /// 
    /// This method:
    /// 1. Backs up current task state (including trapframe) for potential rollback
    /// 2. Opens the binary file and detects the appropriate ABI
    /// 3. Sets up VFS environment and working directory for the target ABI
    /// 4. Delegates execution to the detected ABI module
    /// 5. Restores original state (including trapframe) on failure
    /// 
    /// # Arguments
    /// * `path` - Path to the binary to execute
    /// * `argv` - Command line arguments
    /// * `envp` - Environment variables
    /// * `task` - The task to execute in (will be modified)
    /// * `trapframe` - The trapframe for execution context (will be modified)
    /// 
    /// # Returns
    /// * `Ok(())` on successful execution setup
    /// * `Err(ExecutorError)` if execution setup fails (with task state and trapframe restored)
    pub fn execute_binary(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        task: &mut Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        Self::execute_with_optional_abi(path, argv, envp, None, task, trapframe)
    }

    /// Execute binary with explicit ABI specification
    /// 
    /// This method performs the same operations as execute_binary() but
    /// uses the explicitly specified ABI instead of auto-detection.
    /// Task state and trapframe are backed up and restored on failure.
    pub fn execute_with_abi(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        abi_name: &str,
        task: &mut Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        Self::execute_with_optional_abi(path, argv, envp, Some(abi_name), task, trapframe)
    }

    /// Unified execution implementation with optional ABI specification
    /// 
    /// This method handles both automatic ABI detection and explicit ABI specification
    /// with unified backup/restore logic and error handling.
    fn execute_with_optional_abi(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        explicit_abi: Option<&str>,
        task: &mut Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        // Step 1: Create backup of current task state
        let backup = TaskStateBackup::create_backup(task, trapframe);
        
        // Execute with unified error handling and restoration
        let result = Self::execute_implementation(path, argv, envp, explicit_abi, task, trapframe);
        
        // If execution failed, restore original state
        if result.is_err() {
            if let Err(restore_err) = backup.restore_to_task(task, trapframe) {
                // Log restore error but don't override original error
                crate::early_println!("Warning: Failed to restore task state after exec failure: {}", restore_err);
            }
        }
        
        result
    }

    /// Core execution implementation
    /// 
    /// This method contains the actual execution logic without backup/restore handling.
    fn execute_implementation(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        explicit_abi: Option<&str>,
        task: &mut Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        // Step 1: Open binary file and determine ABI
        let file_object = Self::open_file(path, task)?;
        let abi_name = match explicit_abi {
            Some(name) => name.to_string(),
            None => Self::detect_abi(&file_object, path)?,
        };
        
        // Step 2: Get ABI module instance
        let abi = crate::abi::AbiRegistry::instantiate(&abi_name)
            .ok_or(ExecutorError::UnsupportedAbi(abi_name.clone()))?;

        // Step 3: Check if ABI switch is required
        let abi_switch_required = abi_name != task.abi.as_ref().unwrap().get_name();
        
        if abi_switch_required {
            // Step 4: Setup complete task environment for new ABI (includes VFS, CWD, and handle conversion)
            Self::setup_task_environment(task, &abi)?;
        }
        
        // Step 5: Execute binary through ABI module
        abi.execute_binary(&file_object, argv, envp, task, trapframe)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        
        // Step 6: Update task's ABI if switch occurred
        if abi_switch_required {
            task.abi = Some(abi);
        }
        
        Ok(())
    }

    /// Open binary file through task's VFS
    fn open_file(path: &str, task: &Task) -> ExecutorResult<crate::object::KernelObject> {
        if let Some(vfs) = task.get_vfs() {
            vfs.open(path, 0) // O_RDONLY
                .map_err(|_| ExecutorError::ResourceAllocationFailed)
        } else {
            Err(ExecutorError::ResourceAllocationFailed)
        }
    }

    /// Detect ABI from file object
    fn detect_abi(file_object: &crate::object::KernelObject, path: &str) -> ExecutorResult<String> {
        match crate::abi::AbiRegistry::detect_best_abi(file_object, path) {
            Some((abi_name, _confidence)) => Ok(abi_name),
            None => Err(ExecutorError::UnknownBinaryFormat),
        }
    }

    /// Setup complete task environment for target ABI
    /// 
    /// This method ensures the task has proper VFS, working directory, and handle conversion
    /// for the target ABI. The TransparentExecutor is responsible for:
    /// 1. Providing clean VFS and base VFS references
    /// 2. Verifying that ABI directories exist in base VFS (user should prepare them)
    /// 3. Calling ABI setup methods with proper parameters
    /// 
    /// Design principle: ABI directories (/system/{abi}, /data/config/{abi}) should be
    /// prepared by the user/administrator beforehand as part of system setup.
    fn setup_task_environment(
        task: &mut Task, 
        abi: &Box<dyn crate::abi::AbiModule>
    ) -> ExecutorResult<()> {
        // TransparentExecutor provides clean VFS for ABI environment
        let clean_vfs = Self::create_clean_vfs()
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        
        task.vfs = Some(clean_vfs);
        
        // Get base VFS (global VFS) for overlay and shared resources
        let base_vfs = get_global_vfs_manager();
        
        // Prepare ABI-specific directories in base VFS
        let abi_name = abi.get_name();
        let system_path = alloc::format!("/system/{}", abi_name);
        let config_path = alloc::format!("/data/config/{}", abi_name);
        
        // Verify that ABI directories already exist in base VFS
        // User should have prepared the environment beforehand
        if base_vfs.metadata(&system_path).is_err() {
            return Err(ExecutorError::ExecutionFailed(
                alloc::format!("System directory /system/{} does not exist - please prepare ABI environment first", abi_name)
            ));
        }
        
        if base_vfs.metadata(&config_path).is_err() {
            return Err(ExecutorError::ExecutionFailed(
                alloc::format!("Config directory /data/config/{} does not exist - please prepare ABI environment first", abi_name)
            ));
        }
        
        // Setup ABI-specific environment with the clean VFS
        if let Some(ref vfs_arc) = task.vfs {
            // Step 1: Overlay environment setup with prepared paths
            abi.setup_overlay_environment(vfs_arc, &base_vfs, &system_path, &config_path)
                .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
            
            // Step 2: Shared resources setup with base VFS
            match abi.setup_shared_resources(vfs_arc, &base_vfs) {
                Ok(()) => {}
                Err(e) => {
                    // Log error but do not fail execution - shared resources are optional
                    crate::println!("Warning: Failed to setup shared resources for ABI {}: {}", abi_name, e);
                    Err(ExecutorError::ExecutionFailed(
                        alloc::format!("Failed to setup shared resources for ABI {}: {}", abi_name, e)
                    ))?;
                }
            }
        }
        
        // Set default working directory for the ABI
        task.cwd = Some(abi.get_default_cwd().to_string());
        
        // Let ABI module handle conversion from previous ABI (handles, etc.)
        abi.initialize_from_existing_handles(task)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        
        Ok(())
    }
    
    /// Create a clean VFS with root filesystem
    /// 
    /// The TransparentExecutor is responsible for providing clean VFS instances
    /// that ABI modules can then configure with their specific requirements.
    fn create_clean_vfs() -> Result<Arc<crate::fs::VfsManager>, &'static str> {
        let vfs = crate::fs::VfsManager::new();
        Ok(Arc::new(vfs))
    }
}
