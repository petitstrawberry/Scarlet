//! ABI module.
//!
//! This module provides the interface for ABI (Application Binary Interface) modules
//! in the Scarlet kernel. ABI modules are responsible for handling system calls
//! and providing the necessary functionality for different application binary
//! interfaces.
//!

use crate::sync::{IrqSpinLock, Once};
use crate::{
    arch::Trapframe,
    task::{CloneFlags, mytask},
};
use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use hashbrown::HashMap;
pub mod linux;
pub mod scarlet;
pub mod xv6;

pub const MAX_ABI_LENGTH: usize = 64;

/// Runtime configuration for delegating binary execution to userland
///
/// This structure defines how a binary format should be executed via
/// a userland runtime instead of being loaded directly by the kernel.
///
/// # Examples
///
/// Possible uses of this delegation mechanism, not a list of bundled runtimes:
/// - MS-DOS binaries executed via DOSBox (Linux ABI)
/// - Wasm binaries executed via a Scarlet-native Wasm runtime
/// - Java bytecode executed via a JVM
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Path to the runtime executable in the VFS
    pub runtime_path: String,

    /// ABI that the runtime itself uses
    /// If None, runtime's ABI will be auto-detected
    pub runtime_abi: Option<String>,

    /// Additional arguments to pass to the runtime before the target binary
    /// Example: ["--emulate", "dos"] for DOSBox
    pub runtime_args: Vec<String>,
}

/// ABI module trait.
///
/// This trait defines the interface for ABI modules in the Scarlet kernel.
/// ABI modules are responsible for handling system calls and providing
/// the necessary functionality for different application binary interfaces.
///
/// Each ABI module supplies [`AbiModule::clone_boxed`] for task cloning; the
/// trait does not require Rust's `Clone` trait. Separate ABI instances may still
/// share process-wide state through owned references where the ABI requires it.
///
pub trait AbiModule: Send + Sync + 'static {
    /// Return the name used to register this concrete ABI implementation.
    ///
    /// # Arguments
    /// No arguments; callable for a concrete `Self` type.
    ///
    /// # Returns
    /// The static registry name.
    fn name() -> &'static str
    where
        Self: Sized;

    /// Return this instance's ABI name.
    ///
    /// # Arguments
    /// * `self` - ABI instance to inspect.
    ///
    /// # Returns
    /// An owned name corresponding to its registry entry.
    fn get_name(&self) -> String;

    /// Clone this ABI module into a boxed trait object
    ///
    /// This method enables cloning ABI modules as trait objects,
    /// allowing each task to have its own ABI instance. This need not deeply
    /// copy process-shared state; sharing must follow the ABI's cloning semantics.
    ///
    /// # Arguments
    /// * `self` - ABI instance whose state is to be cloned.
    ///
    /// # Returns
    /// A boxed ABI instance for the child task.
    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync>;

    /// Dispatch a system call according to this ABI's register and error conventions.
    ///
    /// # Arguments
    /// * `trapframe` - Saved user context containing the syscall number and arguments.
    ///
    /// # Returns
    /// The ABI-encoded return value, or a kernel-side dispatch error. An `Ok`
    /// value can itself encode an ABI-level error such as a negative errno.
    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str>;

    /// Hook invoked after Task::clone_task creates the child
    ///
    /// # Arguments
    /// * `parent_task` - Source task for the clone.
    /// * `child_task` - Newly created child task.
    /// * `flags` - Requested resource-sharing and thread-creation flags.
    ///
    /// # Returns
    /// `Ok(())` after ABI-specific child setup, or an error. The default is a no-op.
    fn on_task_cloned(
        &mut self,
        _parent_task: &crate::task::Task,
        _child_task: &crate::task::Task,
        _flags: CloneFlags,
    ) -> Result<(), &'static str> {
        Ok(())
    }

    /// Hook invoked as part of Task::exit cleanup for the current task
    ///
    /// ABI modules can perform per-ABI teardown such as waking futex waiters,
    /// clearing TLS/robust-list pointers, or delivering exit-related signals.
    ///
    /// # Arguments
    /// * `task` - Exiting task whose ABI-local state is being torn down.
    ///
    /// # Returns
    /// No value. The default implementation performs no teardown.
    fn on_task_exit(&mut self, _task: &crate::task::Task) {}

    /// Hook invoked by the current task before it terminates its thread group.
    ///
    /// # Arguments
    ///
    /// * `task` - The current task initiating process-wide teardown.
    ///
    /// # Returns
    ///
    /// This method does not return a value. The default implementation preserves
    /// ABI modules that have no process-shared resources.
    fn on_process_exit(&mut self, _task: &crate::task::Task) {}

    /// Get the task namespace for this ABI.
    ///
    /// This allows each ABI to have its own namespace for task IDs.
    /// By default, returns the root namespace.
    ///
    /// # Arguments
    /// * `self` - ABI instance whose task-ID namespace is requested.
    ///
    /// # Returns
    /// The task namespace for this ABI
    fn get_task_namespace(&self) -> Arc<crate::task::namespace::TaskNamespace> {
        crate::task::namespace::get_root_namespace().clone()
    }

    /// Determine if a binary can be executed by this ABI and return confidence
    ///
    /// This method reads binary content directly from the file object and
    /// executes ABI-specific detection logic (magic bytes, header structure,
    /// entry point validation, etc.).
    /// A detection score is not a substitute for loader validation: explicit ABI
    /// selection bypasses detection, and the file may change before loading.
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
    ///
    /// Pseudocode with illustrative format-checking helpers, not methods provided
    /// by this trait:
    ///
    /// ```text
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
    fn can_execute_binary(
        &self,
        _file_object: &crate::object::KernelObject,
        _file_path: &str,
        _current_abi: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        // Default implementation: cannot determine
        None
    }

    /// Convert inherited handles when switching ABIs.
    ///
    /// The executor calls this on an unpublished image before loading its
    /// binary. Implementations may change that image but must not mutate the
    /// calling process or close its descriptors.
    ///
    /// # Arguments
    ///
    /// * `task` - Task whose inherited handles should be converted.
    ///
    /// # Returns
    ///
    /// `Ok(())` when handle conversion succeeds, or an error when the new ABI
    /// cannot initialize its inherited handle state.
    fn initialize_from_existing_handles(
        &mut self,
        _task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        Ok(()) // Default: no conversion needed
    }

    /// Prepare ABI-owned descriptor state against an unpublished exec image.
    fn prepare_exec_handles(
        &mut self,
        _source: Option<&dyn AbiModule>,
        image: &crate::task::Task,
    ) -> Result<(), &'static str> {
        self.initialize_from_existing_handles(image)
    }

    /// Binary execution (each ABI supports its own binary format)
    ///
    /// This method loads a binary and prepares its execution context. Use
    /// `file_object.as_file()` to access `FileObject`, and invoke the appropriate
    /// ABI-specific loader. The loader must validate the format, bounds, and CPU
    /// architecture itself: `can_execute_binary` is a detection heuristic and is
    /// not called when an ABI is selected explicitly. Returning success does not
    /// itself enter userspace; the caller resumes the prepared context later.
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
    /// # Returns
    /// `Ok(())` after preparing the new image and registers, or a loader error.
    /// An error does not by itself guarantee rollback of partially modified state.
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
    /// 1. sys_execve() calls this method through TransparentExecutor
    /// 2. sys_execve() returns usize to syscall_handler()
    /// 3. syscall_handler() returns Ok(usize) to syscall_dispatcher()
    /// 4. syscall_dispatcher() returns Ok(usize) to trap handler
    /// 5. Trap handler calls trapframe.set_return_value(usize) automatically
    ///
    /// On success, the native `sys_execve()` preserves the return-value register
    /// prepared by the loader by returning `trapframe.get_return_value()`, rather
    /// than overwriting a new program's initial register value with zero.
    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &crate::task::Task,
        trapframe: &mut Trapframe,
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
    fn choose_load_address(
        &self,
        _elf_type: u16,
        _target: crate::task::elf_loader::LoadTarget,
    ) -> Option<u64> {
        None // Default: use kernel default strategy
    }

    /// Get userland runtime configuration for executing binaries
    ///
    /// This method allows ABI modules to delegate binary execution to userland runtimes.
    /// When a runtime is configured, the binary will be executed via the runtime instead
    /// of being loaded directly by the kernel.
    ///
    /// # Arguments
    /// * `file_object` - Binary file to check
    /// * `file_path` - File path for format detection
    ///
    /// # Returns
    /// * `Some(RuntimeConfig)` - Runtime configuration if delegation is needed
    /// * `None` - No runtime delegation, execute directly
    ///
    /// # Example Use Cases
    ///
    /// These are extension possibilities, not claims that the runtimes are bundled:
    /// - MS-DOS binaries via DOSBox (Linux ABI runtime)
    /// - Wasm binaries via Scarlet-native Wasm runtime
    /// - Java bytecode via JVM
    /// - Cross-architecture binaries via QEMU user-mode
    fn get_runtime_config(
        &self,
        _file_object: &crate::object::KernelObject,
        _file_path: &str,
    ) -> Option<RuntimeConfig> {
        None // Default: no runtime delegation
    }

    /// Handle incoming event from EventManager
    ///
    /// This method is called when an event is delivered to a task using this ABI.
    /// Each ABI can implement its own event handling strategy. Possible strategies
    /// include the following; they are not guarantees made by this default hook:
    /// - Scarlet ABI: Handle-based queuing with EventSubscription objects
    /// - xv6 ABI: POSIX-like signals and pipe notifications
    /// - Other ABIs: Custom event processing mechanisms
    ///
    /// # Arguments
    /// * `event` - The event to be delivered
    /// * `target_task_id` - Global kernel ID of the task that should receive the event
    ///
    /// # Returns
    /// * `Ok(outcome)` describing how event processing should proceed
    /// * `Err(message)` if event delivery failed
    ///
    /// The default ignores the event and returns `Ok(EventProcessOutcome::Continue)`.
    fn handle_event(
        &mut self,
        _event: crate::ipc::Event,
        _target_task_id: usize,
    ) -> Result<EventProcessOutcome, &'static str> {
        // Default implementation: ignore events
        Ok(EventProcessOutcome::Continue)
    }

    /// Set the TLS (Thread Local Storage) pointer for this task
    ///
    /// Default implementation does nothing - ABIs that support TLS
    /// should override this method.
    ///
    /// # Arguments
    /// * `ptr` - ABI-specific user TLS pointer to record for this task.
    ///
    /// # Returns
    /// No value. Recording a pointer does not validate its user memory.
    fn set_tls_pointer(&mut self, _ptr: usize) {
        // Default: do nothing
    }

    /// Get the TLS (Thread Local Storage) pointer for this task
    ///
    /// Default implementation returns None - ABIs that support TLS
    /// should override this method.
    ///
    /// # Arguments
    /// * `self` - ABI instance whose recorded TLS pointer is requested.
    ///
    /// # Returns
    /// The recorded pointer, or `None` when unavailable; this is not a memory-access check.
    fn get_tls_pointer(&self) -> Option<usize> {
        None
    }

    /// Set the clear_child_tid pointer for thread exit notification
    ///
    /// Default implementation does nothing - ABIs that support clear-child-TID
    /// exit notifications should override this method.
    ///
    /// # Arguments
    /// * `ptr` - User address used by the ABI for thread-exit notification.
    ///
    /// # Returns
    /// No value. An override must validate user memory when accessing the address.
    fn set_clear_child_tid(&mut self, _ptr: usize) {
        // Default: do nothing
    }

    /// Get a reference to Any for downcasting
    ///
    /// This allows code to downcast the AbiModule to a concrete type
    /// to access ABI-specific functionality.
    ///
    /// # Arguments
    /// * `self` - ABI instance to inspect.
    ///
    /// # Returns
    /// A type-erased shared borrow when implemented by the ABI.
    ///
    /// # Panics
    /// The default implementation always panics; ABIs supporting downcasting must override it.
    fn as_any(&self) -> &dyn core::any::Any {
        panic!("as_any not implemented for this ABI")
    }

    /// Get a mutable reference to Any for downcasting
    ///
    /// This allows code to downcast the AbiModule to a concrete type
    /// to access ABI-specific functionality.
    ///
    /// # Arguments
    /// * `self` - Exclusively borrowed ABI instance.
    ///
    /// # Returns
    /// A type-erased exclusive borrow when implemented by the ABI.
    ///
    /// # Panics
    /// The default implementation always panics; ABIs supporting downcasting must override it.
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        panic!("as_any_mut not implemented for this ABI")
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

    pub fn global() -> &'static IrqSpinLock<AbiRegistry> {
        // Thread-safe lazy initialization using Once
        static INSTANCE: Once<IrqSpinLock<AbiRegistry>> = Once::new();

        INSTANCE.call_once(|| IrqSpinLock::new(AbiRegistry::new()))
    }

    pub fn register<T>()
    where
        T: AbiModule + Default + 'static,
    {
        crate::println!("Registering ABI module: {}", T::name());
        let mut registry = Self::global().lock();
        registry
            .factories
            .insert(T::name().to_string(), || Box::new(T::default()));
    }

    pub fn instantiate(name: &str) -> Option<Box<dyn AbiModule + Send + Sync>> {
        let factory = Self::global().lock().factories.get(name).copied();
        factory.map(|factory| factory())
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
    pub fn detect_best_abi(
        file_object: &crate::object::KernelObject,
        file_path: &str,
    ) -> Option<(String, u8)> {
        // Detection reads executable data. Never retain an IRQ/preemption lock
        // across ABI callbacks or backing-filesystem I/O.
        let factories: Vec<_> = Self::global()
            .lock()
            .factories
            .iter()
            .map(|(name, factory)| (name.clone(), *factory))
            .collect();

        // Get current task's ABI reference for inheritance consideration
        let _task = mytask();

        // Try all ABI modules and find the one with highest confidence
        // Each ABI decides its own confidence based on:
        // - Binary format compatibility
        // - Architecture compatibility
        // - Entry point validity
        // - Inheritance bonus from current ABI
        if let Some(ref task) = _task {
            task.with_default_abi(|current_abi| {
                factories
                    .iter()
                    .filter_map(|(name, factory)| {
                        let abi = factory();
                        abi.can_execute_binary(file_object, file_path, Some(current_abi))
                            .map(|confidence| (name.clone(), confidence))
                    })
                    .max_by_key(|(_, confidence)| *confidence)
            })
        } else {
            factories
                .iter()
                .filter_map(|(name, factory)| {
                    let abi = factory();
                    abi.can_execute_binary(file_object, file_path, None)
                        .map(|confidence| (name.clone(), confidence))
                })
                .max_by_key(|(_, confidence)| *confidence)
        }
    }
}

#[macro_export]
macro_rules! register_abi {
    ($ty:ty) => {
        $crate::abi::AbiRegistry::register::<$ty>();
    };
}

/// Result of processing one queued event for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventProcessOutcome {
    /// The event was handled in kernel space; event processing may continue.
    Continue,
    /// The event is still pending because it is masked.
    Pending,
    /// A userspace handler frame has been armed; return to userspace immediately.
    UserHandlerArmed,
    /// The task state changed so the scheduler must pick another task.
    NeedReschedule,
    /// The task must exit with this status after the ABI mutable borrow is released.
    Exited(i32),
}

pub fn syscall_dispatcher(trapframe: &mut Trapframe) -> Result<usize, &'static str> {
    // 1. Get the program counter (sepc) from trapframe
    let pc = trapframe.get_current_pc() as usize;
    let syscall_number = trapframe.get_syscall_number();
    crate::breadcrumb::drop(
        crate::breadcrumb::SYSCALL_ENTER,
        syscall_number as u64,
        pc as u64,
    );

    // 2. Get mutable reference to current task
    let task = mytask().unwrap();
    task.record_syscall_entry(syscall_number, pc);
    crate::breadcrumb::drop(
        crate::breadcrumb::SYSCALL_TASK_DONE,
        task.get_id() as u64,
        syscall_number as u64,
    );

    // 3. Resolve the appropriate ABI based on PC address and handle the syscall
    let res = task.with_resolve_abi_mut(pc, |abi_module| {
        // 4. Handle the system call with the resolved ABI
        abi_module.handle_syscall(trapframe)
    });
    crate::breadcrumb::drop(
        crate::breadcrumb::SYSCALL_ABI_DONE,
        task.get_id() as u64,
        syscall_number as u64,
    );
    task.finish_exec_dispatch();
    task.process_deferred_exit_request();
    task.record_syscall_exit();
    crate::breadcrumb::drop(crate::breadcrumb::SYSCALL_EXIT, syscall_number as u64, 0);
    res
}
