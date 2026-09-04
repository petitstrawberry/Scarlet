//! TransparentExecutor Implementation
//!
//! The TransparentExecutor provides unified exec API for all ABIs.
//! It does NOT contain ABI-specific knowledge - each ABI module handles
//! its own binary format and conversion logic.

use crate::arch::{Trapframe, vcpu::Vcpu};
use crate::fs::VfsManager;
use crate::mem::page::{ContiguousPages, TaskPages};
use crate::vm::vmem::VirtualMemoryMap;
use crate::{fs::manager::get_global_vfs_manager, task::Task};
use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::fmt;
use core::sync::atomic::Ordering;

/// Task state backup for exec rollback
///
/// This structure contains a complete backup of task state that can be
/// restored if execve fails. Includes memory state, metadata, and trapframe.
struct TaskStateBackup {
    page_allocations: Vec<ContiguousPages>,
    task_pages: Vec<TaskPages>,
    vm_mapping: Vec<VirtualMemoryMap>,
    text_size: usize,
    data_size: usize,
    stack_size: usize,
    brk: usize,
    name: String,
    vcpu: Vcpu,
    vfs: Option<Arc<VfsManager>>,
    trapframe: Trapframe,
}

impl TaskStateBackup {
    fn create_backup(task: &Task, trapframe: &Trapframe) -> Self {
        // Move all page allocations out of the task into the backup, leaving an empty Vec
        // inside the task. This preserves move-based semantics and avoids allocating new
        // physical pages via Clone.
        let backup_pages = {
            let mut pages_guard = task.page_allocations.write();
            core::mem::take(&mut *pages_guard)
        };
        let backup_task_pages = {
            let mut pages_guard = task.task_pages.write();
            core::mem::take(&mut *pages_guard)
        };

        let backup_vm_mapping = task.vm_manager.remove_all_memory_maps().collect();

        Self {
            page_allocations: backup_pages,
            task_pages: backup_task_pages,
            vm_mapping: backup_vm_mapping,
            text_size: task.text_size.load(Ordering::SeqCst),
            data_size: task.data_size.load(Ordering::SeqCst),
            stack_size: task.stack_size.load(Ordering::SeqCst),
            brk: task.brk.load(Ordering::SeqCst),
            name: task.name.read().clone(),
            vcpu: task.vcpu.lock().clone(),
            vfs: task.vfs.read().clone(),
            trapframe: trapframe.clone(),
        }
    }

    fn restore_to_task(self, task: &Task, trapframe: &mut Trapframe) -> Result<(), &'static str> {
        task.vm_manager.restore_memory_maps(self.vm_mapping)?;

        let partial_page_allocations = {
            let mut pages = task.page_allocations.write();
            core::mem::replace(&mut *pages, self.page_allocations)
        };
        let partial_task_pages = {
            let mut pages = task.task_pages.write();
            core::mem::replace(&mut *pages, self.task_pages)
        };
        drop(partial_page_allocations);
        drop(partial_task_pages);

        task.text_size.store(self.text_size, Ordering::SeqCst);
        task.data_size.store(self.data_size, Ordering::SeqCst);
        task.stack_size.store(self.stack_size, Ordering::SeqCst);
        task.brk.store(self.brk, Ordering::SeqCst);
        *task.name.write() = self.name;
        *task.vcpu.lock() = self.vcpu;
        *task.vfs.write() = self.vfs;

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
    /// Binary file could not be opened
    OpenFailed(String),
    /// Execution failed
    ExecutionFailed(String),
}

impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutorError::UnknownBinaryFormat => write!(f, "Unknown binary format"),
            ExecutorError::UnsupportedAbi(abi) => write!(f, "Unsupported ABI: {}", abi),
            ExecutorError::OpenFailed(path) => write!(f, "Failed to open executable: {}", path),
            ExecutorError::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
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
    /// * `force_abi_rebuild` - Flag to force ABI environment reconstruction
    ///
    /// # Returns
    /// * `Ok(())` on successful execution setup
    /// * `Err(ExecutorError)` if execution setup fails (with task state and trapframe restored)
    ///
    pub fn execute_binary(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        task: &Task,
        trapframe: &mut Trapframe,
        force_abi_rebuild: bool,
    ) -> ExecutorResult<()> {
        Self::execute_with_optional_abi(path, argv, envp, None, task, trapframe, force_abi_rebuild)
    }

    /// Execute binary with explicit ABI specification and flags
    ///
    /// This method extends `execute_binary()` to support additional flags,
    /// particularly for forcing ABI environment reconstruction.
    ///
    /// # Arguments
    /// * `path` - Path to the binary to execute
    /// * `argv` - Command line arguments
    /// * `envp` - Environment variables
    /// * `abi_name` - Name of the ABI to use
    /// * `task` - The task to execute in (will be modified)
    /// * `trapframe` - The trapframe for execution context (will be modified)
    /// * `force_abi_rebuild` - Flag to force ABI environment reconstruction
    ///
    /// # Returns
    /// * `Ok(())` on successful execution setup
    /// * `Err(ExecutorError)` if execution setup fails (with task state and trapframe restored)
    ///
    pub fn execute_with_abi(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        abi_name: &str,
        task: &Task,
        trapframe: &mut Trapframe,
        force_abi_rebuild: bool,
    ) -> ExecutorResult<()> {
        Self::execute_with_optional_abi(
            path,
            argv,
            envp,
            Some(abi_name),
            task,
            trapframe,
            force_abi_rebuild,
        )
    }

    /// Unified execution implementation with optional ABI specification and flags
    ///
    /// This method handles both automatic ABI detection and explicit ABI specification
    /// with unified backup/restore logic and error handling.
    fn execute_with_optional_abi(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        explicit_abi: Option<&str>,
        task: &Task,
        trapframe: &mut Trapframe,
        force_abi_rebuild: bool,
    ) -> ExecutorResult<()> {
        // Step 1: Create backup of current task state
        let backup = TaskStateBackup::create_backup(task, trapframe);

        // Execute with unified error handling and restoration
        let result = Self::execute_implementation(
            path,
            argv,
            envp,
            explicit_abi,
            task,
            trapframe,
            force_abi_rebuild,
        );

        // If execution failed, restore original state
        if result.is_err() {
            if let Err(restore_err) = backup.restore_to_task(task, trapframe) {
                // Log restore error but don't override original error
                crate::println!(
                    "Warning: Failed to restore task state after exec failure: {}",
                    restore_err
                );
            }
        }

        result
    }

    /// Core execution implementation with flags support
    ///
    /// This method contains the actual execution logic without backup/restore handling.
    fn execute_implementation(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        explicit_abi: Option<&str>,
        task: &Task,
        trapframe: &mut Trapframe,
        force_abi_rebuild: bool,
    ) -> ExecutorResult<()> {
        // Step 1: Open binary file and determine ABI
        let file_object = Self::open_file(path, task)?;
        let abi_name = match explicit_abi {
            Some(name) => name.to_string(),
            None => Self::detect_abi(&file_object, path)?,
        };

        // Step 2: Get ABI module instance
        let mut abi = crate::abi::AbiRegistry::instantiate(&abi_name)
            .ok_or(ExecutorError::UnsupportedAbi(abi_name.clone()))?;

        // Step 3: Check if runtime delegation is needed
        if let Some(runtime_config) = abi.get_runtime_config(&file_object, path) {
            // Delegate execution to userland runtime
            return Self::execute_via_runtime(
                path,
                argv,
                envp,
                &runtime_config,
                task,
                trapframe,
                force_abi_rebuild,
            );
        }

        // Step 4: Check if ABI switch or forced rebuild is required
        let current_abi_name = task.with_default_abi(|abi| abi.get_name());
        let abi_switch_required = abi_name != current_abi_name;
        let rebuild_required = abi_switch_required || force_abi_rebuild;

        if rebuild_required {
            // Step 5: Setup complete task environment for new ABI (includes VFS, CWD)
            // crate::println!(
            //     "[TransparentExecutor] Setting up environment for ABI: {}",
            //     abi_name
            // );
            Self::setup_task_environment(task, &mut abi)?;
        }

        // Step 6: Execute binary through ABI module (pass envp directly)
        abi.execute_binary(&file_object, argv, envp, task, trapframe)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;

        // Step 7: Convert inherited handles only after the new image is loaded.
        // Some ABIs destructively rebuild handle state, which must not happen
        // before an ELF loading failure can return to the previous image.
        if rebuild_required {
            abi.initialize_from_existing_handles(task)
                .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        }

        // Step 8: Update task's ABI if switch occurred
        if abi_switch_required {
            // SAFETY: This is the currently executing task on this hart
            unsafe {
                *task.default_abi.get_mut() = Some(abi);
            }
        }

        // Linux-visible launchers can use a namespace-relative absolute
        // argv[0] even when Scarlet opened the image through a host-visible
        // bundle path. Prefer that stable executable identity for `/proc/*/exe`
        // and keep the actual open path as the fallback.
        let executable_path = argv
            .first()
            .copied()
            .filter(|arg| arg.starts_with('/'))
            .unwrap_or(path);
        task.set_executable_path(executable_path);

        Ok(())
    }

    /// Open binary file through task's VFS
    ///
    /// TODO: Improve VFS API to handle relative paths natively
    /// Current implementation manually resolves relative paths, but this should
    /// be handled by VFS layer for consistency and better error handling.
    fn open_file(path: &str, task: &Task) -> ExecutorResult<crate::object::KernelObject> {
        if let Some(vfs) = task.get_vfs() {
            let absolute_path = vfs.resolve_path_to_absolute(path);

            match vfs.open(&absolute_path, 0) {
                // O_RDONLY
                Ok(obj) => Ok(obj),
                Err(_) => Err(ExecutorError::OpenFailed(absolute_path)),
            }
        } else {
            Err(ExecutorError::OpenFailed(path.to_string()))
        }
    }

    /// Detect ABI from file object
    fn detect_abi(file_object: &crate::object::KernelObject, path: &str) -> ExecutorResult<String> {
        match crate::abi::AbiRegistry::detect_best_abi(file_object, path) {
            Some((abi_name, _confidence)) => Ok(abi_name),
            None => Err(ExecutorError::UnknownBinaryFormat),
        }
    }

    /// Execute binary via userland runtime
    ///
    /// This method delegates binary execution to a userland runtime, enabling:
    /// - Cross-architecture emulation (e.g., MS-DOS via DOSBox)
    /// - Alternative runtime environments (e.g., Wasm, Java bytecode)
    /// - Complex runtimes in userspace without kernel bloat
    ///
    /// # Arguments
    /// * `target_path` - Path to the binary to execute
    /// * `target_argv` - Command line arguments for the target binary
    /// * `target_envp` - Environment variables for the target binary
    /// * `runtime_config` - Runtime configuration (path, ABI, args)
    /// * `task` - The task to execute in
    /// * `trapframe` - The trapframe for execution context
    /// * `force_abi_rebuild` - Flag to force ABI environment reconstruction
    ///
    /// # Returns
    /// * `Ok(())` on successful runtime execution setup
    /// * `Err(ExecutorError)` if runtime execution fails
    ///
    /// # Execution Flow
    /// 1. Construct runtime arguments: [runtime_args..., target_path, target_argv...]
    /// 2. If runtime_abi is specified, use execute_with_abi
    /// 3. Otherwise, auto-detect runtime's ABI and execute
    fn execute_via_runtime(
        target_path: &str,
        target_argv: &[&str],
        target_envp: &[&str],
        runtime_config: &crate::abi::RuntimeConfig,
        task: &Task,
        trapframe: &mut Trapframe,
        force_abi_rebuild: bool,
    ) -> ExecutorResult<()> {
        // Build runtime arguments: [runtime_args..., target_path, target_argv...]
        let mut runtime_argv = Vec::new();

        // Add runtime executable name as argv[0]
        runtime_argv.push(runtime_config.runtime_path.as_str());

        // Add configured runtime arguments
        for arg in &runtime_config.runtime_args {
            runtime_argv.push(arg.as_str());
        }

        // Add target binary path
        runtime_argv.push(target_path);

        // Add target binary arguments (skip argv[0] which is the target binary name)
        for arg in target_argv.iter().skip(1) {
            runtime_argv.push(*arg);
        }

        crate::println!(
            "[Runtime Delegation] Executing '{}' via runtime '{}'",
            target_path,
            runtime_config.runtime_path
        );

        // Execute runtime with constructed arguments
        match &runtime_config.runtime_abi {
            Some(abi_name) => {
                // Explicit ABI specified for runtime
                Self::execute_with_optional_abi(
                    &runtime_config.runtime_path,
                    &runtime_argv,
                    target_envp,
                    Some(abi_name.as_str()),
                    task,
                    trapframe,
                    force_abi_rebuild,
                )
            }
            None => {
                // Auto-detect runtime's ABI
                Self::execute_with_optional_abi(
                    &runtime_config.runtime_path,
                    &runtime_argv,
                    target_envp,
                    None,
                    task,
                    trapframe,
                    force_abi_rebuild,
                )
            }
        }
    }

    /// Setup complete task environment for target ABI
    ///
    /// This method ensures the task has a proper VFS and working directory
    /// for the target ABI. The TransparentExecutor is responsible for:
    /// 1. Providing clean VFS and base VFS references
    /// 2. Verifying that ABI directories exist in base VFS (user should prepare them)
    /// 3. Calling ABI setup methods with proper parameters
    ///
    /// Design principle: ABI directories (/system/{abi}, /data/config/{abi}) should be
    /// prepared by the user/administrator beforehand as part of system setup.
    fn setup_task_environment(
        task: &Task,
        abi: &mut Box<dyn crate::abi::AbiModule + Send + Sync>,
    ) -> ExecutorResult<()> {
        // TransparentExecutor provides clean VFS for ABI environment
        let clean_vfs =
            Self::create_clean_vfs().map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;

        *task.vfs.write() = Some(clean_vfs);

        // Get base VFS (global VFS) for overlay and shared resources
        let base_vfs = get_global_vfs_manager();

        // Prepare ABI-specific directories in base VFS
        let abi_name = abi.get_name();
        let system_path = alloc::format!("/system/{}", abi_name);
        let config_path = alloc::format!("/data/config/{}", abi_name);

        // Verify that ABI directories already exist in base VFS
        // User should have prepared the environment beforehand
        if base_vfs.metadata(&system_path).is_err() {
            return Err(ExecutorError::ExecutionFailed(alloc::format!(
                "System directory /system/{} does not exist - please prepare ABI environment first",
                abi_name
            )));
        }

        if base_vfs.metadata(&config_path).is_err() {
            return Err(ExecutorError::ExecutionFailed(alloc::format!(
                "Config directory /data/config/{} does not exist - please prepare ABI environment first",
                abi_name
            )));
        }

        // Setup ABI-specific environment with the clean VFS
        if let Some(ref vfs_arc) = *task.vfs.read() {
            // Step 1: Overlay environment setup with prepared paths
            abi.setup_overlay_environment(vfs_arc, &base_vfs, &system_path, &config_path)
                .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;

            // Step 2: Shared resources setup with base VFS
            match abi.setup_shared_resources(vfs_arc, &base_vfs) {
                Ok(()) => {}
                Err(e) => {
                    // Log error but do not fail execution - shared resources are optional
                    crate::println!(
                        "Warning: Failed to setup shared resources for ABI {}: {}",
                        abi_name,
                        e
                    );
                    Err(ExecutorError::ExecutionFailed(alloc::format!(
                        "Failed to setup shared resources for ABI {}: {}",
                        abi_name,
                        e
                    )))?;
                }
            }
        }

        // Set default working directory for the ABI via VfsManager
        if let Some(vfs) = task.vfs.read().clone() {
            let _ = vfs.set_cwd_by_path(abi.get_default_cwd());
        }

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

#[cfg(test)]
mod rollback_tests {
    use core::sync::atomic::Ordering;

    use super::*;
    use crate::environment::PAGE_SIZE;
    use crate::task::new_user_task;
    use crate::vm::vmem::MemoryArea;

    fn memory_map(paddr: usize) -> VirtualMemoryMap {
        VirtualMemoryMap::new(
            MemoryArea::new(paddr, paddr + PAGE_SIZE - 1),
            MemoryArea::new(0x20_0000, 0x20_0000 + PAGE_SIZE - 1),
            0,
            false,
            None,
        )
    }

    #[test_case]
    fn rollback_replaces_partial_exec_memory_and_context() {
        let task = new_user_task("exec-original".to_string(), 0);
        let original_page = ContiguousPages::new(1).expect("original page allocation failed");
        let original_paddr = original_page.as_paddr();
        task.page_allocations.write().push(original_page);
        let original_task_pages = TaskPages::new(1).expect("original task pages allocation failed");
        let original_task_page_paddr = original_task_pages.page_paddr(0).unwrap();
        task.task_pages.write().push(original_task_pages);
        task.vm_manager
            .add_memory_map(memory_map(original_paddr))
            .unwrap();
        task.text_size.store(PAGE_SIZE, Ordering::SeqCst);
        task.data_size.store(PAGE_SIZE * 2, Ordering::SeqCst);
        task.stack_size.store(PAGE_SIZE * 3, Ordering::SeqCst);
        task.brk.store(0x40_0000, Ordering::SeqCst);
        task.vcpu.lock().set_pc(0x20_0100);
        let original_vfs = Arc::new(VfsManager::new());
        *task.vfs.write() = Some(Arc::clone(&original_vfs));
        let mut trapframe = Trapframe::new();
        trapframe.set_pc(0x20_0200);

        let backup = TaskStateBackup::create_backup(&task, &trapframe);

        let partial_page = ContiguousPages::new(1).expect("partial page allocation failed");
        let partial_paddr = partial_page.as_paddr();
        task.page_allocations.write().push(partial_page);
        task.task_pages
            .write()
            .push(TaskPages::new(1).expect("partial task pages allocation failed"));
        task.vm_manager
            .add_memory_map(memory_map(partial_paddr))
            .unwrap();
        task.text_size.store(1, Ordering::SeqCst);
        task.data_size.store(2, Ordering::SeqCst);
        task.stack_size.store(3, Ordering::SeqCst);
        task.brk.store(4, Ordering::SeqCst);
        *task.name.write() = "exec-partial".to_string();
        task.vcpu.lock().set_pc(0xdead);
        *task.vfs.write() = None;
        trapframe.set_pc(0xbeef);

        backup.restore_to_task(&task, &mut trapframe).unwrap();

        assert_eq!(task.vm_manager.memmap_len(), 1);
        assert_eq!(
            task.vm_manager
                .search_memory_map(0x20_0000)
                .unwrap()
                .pmarea
                .start,
            original_paddr
        );
        assert_eq!(task.page_allocations.read().len(), 1);
        assert_eq!(task.page_allocations.read()[0].as_paddr(), original_paddr);
        assert_eq!(task.task_pages.read().len(), 1);
        assert_eq!(
            task.task_pages.read()[0].page_paddr(0),
            Some(original_task_page_paddr)
        );
        assert_eq!(task.text_size.load(Ordering::SeqCst), PAGE_SIZE);
        assert_eq!(task.data_size.load(Ordering::SeqCst), PAGE_SIZE * 2);
        assert_eq!(task.stack_size.load(Ordering::SeqCst), PAGE_SIZE * 3);
        assert_eq!(task.brk.load(Ordering::SeqCst), 0x40_0000);
        assert_eq!(*task.name.read(), "exec-original");
        assert_eq!(task.vcpu.lock().get_pc(), 0x20_0100);
        assert!(
            task.vfs
                .read()
                .as_ref()
                .is_some_and(|vfs| Arc::ptr_eq(vfs, &original_vfs))
        );
        assert_eq!(trapframe.get_current_pc(), 0x20_0200);
    }
}
