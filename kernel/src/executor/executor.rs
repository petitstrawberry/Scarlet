//! TransparentExecutor Implementation
//!
//! The TransparentExecutor provides unified exec API for all ABIs.
//! It does NOT contain ABI-specific knowledge - each ABI module handles
//! its own binary format and conversion logic.

use crate::task::Task;
use crate::arch::Trapframe;
use crate::vm::vmem::VirtualMemoryMap;
use crate::task::ManagedPage;
use alloc::{string::{String, ToString}, vec::Vec};
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
    /// 2. Analyzes the binary to detect the appropriate ABI
    /// 3. Delegates execution to the detected ABI module
    /// 4. Handles VFS inheritance and resource management
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
        // Step 1: Create backup of current task state
        let backup = TaskStateBackup::create_backup(task, trapframe);
        
        // Execute with error handling and restoration
        let result = Self::execute_binary_inner(path, argv, envp, task, trapframe);
        
        // If execution failed, restore original state
        if result.is_err() {
            if let Err(restore_err) = backup.restore_to_task(task, trapframe) {
                // Log restore error but don't override original error
                crate::println!("Warning: Failed to restore task state after exec failure: {}", restore_err);
            }
        }
        
        result
    }
    
    /// Internal execution implementation without state backup
    fn execute_binary_inner(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        task: &mut Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        // Step 1: Open binary file through unified VFS
        let file_object = Self::open_binary_file(path, task)?;
        
        // Step 2: Detect ABI from binary format using file object
        let abi_name = Self::detect_abi_from_file(&file_object, path)?;
        
        // Step 3: Get ABI module instance
        let abi = crate::abi::AbiRegistry::instantiate(&abi_name)
            .ok_or(ExecutorError::UnsupportedAbi(abi_name.clone()))?;
        
        // Step 4: Prepare VFS inheritance (extract shared VFS info)
        Self::prepare_vfs_inheritance(task)?;
        
        // Step 5: Let ABI module handle its own conversion and execution
        abi.initialize_from_existing_handles(task)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        
        // Step 6: Execute binary through ABI module
        abi.execute_binary(&file_object, argv, envp, task, trapframe)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        
        // Step 7: Update task's ABI
        task.abi = Some(abi);
        
        Ok(())
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
        // Step 1: Create backup of current task state
        let backup = TaskStateBackup::create_backup(task, trapframe);
        
        // Execute with error handling and restoration
        let result = Self::execute_with_abi_inner(path, argv, envp, abi_name, task, trapframe);
        
        // If execution failed, restore original state
        if result.is_err() {
            if let Err(restore_err) = backup.restore_to_task(task, trapframe) {
                // Log restore error but don't override original error
                crate::early_println!("Warning: Failed to restore task state after exec failure: {}", restore_err);
            }
        }
        
        result
    }
    
    /// Internal ABI-specific execution implementation without state backup
    fn execute_with_abi_inner(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        abi_name: &str,
        task: &mut Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        // Step 1: Open binary file
        let file_object = Self::open_binary_file(path, task)?;
        
        // Step 2: Get ABI module instance
        let abi = crate::abi::AbiRegistry::instantiate(abi_name)
            .ok_or(ExecutorError::UnsupportedAbi(abi_name.to_string()))?;
        
        // Step 3: Prepare VFS inheritance
        Self::prepare_vfs_inheritance(task)?;
        
        // Step 4: Let ABI module handle conversion and execution
        abi.initialize_from_existing_handles(task)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        
        abi.execute_binary(&file_object, argv, envp, task, trapframe)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        
        // Step 5: Update task's ABI
        task.abi = Some(abi);
        
        Ok(())
    }

    /// Prepare VFS inheritance for exec
    /// 
    /// This extracts shared VFS information that should be inherited
    /// across the exec boundary.
    fn prepare_vfs_inheritance(_task: &Task) -> ExecutorResult<()> {
        // TODO: Extract shared VFS mounts and prepare BaseVfs
        // For now, this is a placeholder
        Ok(())
    }

    /// Open binary file through unified VFS
    fn open_binary_file(path: &str, task: &Task) -> ExecutorResult<crate::object::KernelObject> {
        if let Some(vfs) = &task.vfs {
            vfs.open(path, 0) // O_RDONLY
                .map_err(|_| ExecutorError::ResourceAllocationFailed)
        } else {
            Err(ExecutorError::ResourceAllocationFailed)
        }
    }

    /// Detect ABI from file object
    fn detect_abi_from_file(file_object: &crate::object::KernelObject, path: &str) -> ExecutorResult<String> {
        // Use ABI registry to detect the best ABI directly from file object
        match crate::abi::AbiRegistry::detect_best_abi(file_object, path) {
            Some((abi_name, _confidence)) => Ok(abi_name),
            None => Err(ExecutorError::UnknownBinaryFormat),
        }
    }
}
