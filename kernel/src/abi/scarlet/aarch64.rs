//! Scarlet Native ABI Module (AArch64)
//!
//! This module implements the Scarlet ABI for the Scarlet kernel.
//! It provides the necessary functionality for handling system calls
//! and interacting with the Scarlet kernel.

use alloc::{
    boxed::Box,
    collections::btree_map::BTreeMap,
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::sync::atomic::Ordering;

use crate::{
    arch::{Trapframe, vm},
    fs::{
        FileSystemError, FileSystemErrorKind, SeekFrom, VfsManager, drivers::overlayfs::OverlayFS,
    },
    ipc::event::{Event, EventContent, EventPriority, ProcessControlType},
    late_initcall, register_abi,
    syscall::syscall_handler,
    task::elf_loader::{
        ExecutionMode, LoadStrategy, LoadTarget, analyze_and_load_elf_with_strategy,
        build_auxiliary_vector, setup_auxiliary_vector_on_stack,
    },
    vm::setup_user_stack,
};

use crate::abi::AbiModule;

/// Maximum number of pending events that can be queued
/// When this limit is reached, oldest events are dropped
const MAX_PENDING_EVENTS: usize = 1024;

/// Event handler function pointer type (user-space address)
pub type EventHandler = usize;

/// Event handler registration entry
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventHandlerEntry {
    /// Handler function address in user space
    pub handler: EventHandler,
    /// Whether this handler should be called synchronously
    pub synchronous: bool,
}

/// Event mask for filtering/blocking events
#[derive(Debug, Clone, Default)]
pub struct EventMask {
    /// Blocked event content types (ProcessControl types)
    pub blocked_process_control: u32,
    /// Blocked notification types
    pub blocked_notifications: u64,
    /// Blocked custom event namespaces
    pub blocked_namespaces: Vec<String>,
    /// Block all events flag
    pub block_all: bool,
}

impl EventMask {
    /// Create a new empty event mask (no events blocked)
    pub fn new() -> Self {
        Self::default()
    }

    /// Block all events
    pub fn block_all(&mut self) {
        self.block_all = true;
    }

    /// Unblock all events
    pub fn unblock_all(&mut self) {
        self.block_all = false;
        self.blocked_process_control = 0;
        self.blocked_notifications = 0;
        self.blocked_namespaces.clear();
    }

    /// Block a specific ProcessControlType
    pub fn block_process_control(&mut self, ptype: ProcessControlType) {
        let bit = match ptype {
            ProcessControlType::Terminate => 0,
            ProcessControlType::Kill => 1,
            ProcessControlType::Stop => 2,
            ProcessControlType::Continue => 3,
            ProcessControlType::Interrupt => 4,
            ProcessControlType::Quit => 5,
            ProcessControlType::Hangup => 6,
            ProcessControlType::ChildExit => 7,
            ProcessControlType::PipeBroken => 8,
            ProcessControlType::Alarm => 9,
            ProcessControlType::IoReady => 10,
            ProcessControlType::User(n) => {
                // Constrain user signals to 0-20 to avoid collisions
                // User signals beyond 20 are treated as 20
                11 + n.min(20)
            }
        };
        self.blocked_process_control |= 1 << bit;
    }

    /// Unblock a specific ProcessControlType
    pub fn unblock_process_control(&mut self, ptype: ProcessControlType) {
        let bit = match ptype {
            ProcessControlType::Terminate => 0,
            ProcessControlType::Kill => 1,
            ProcessControlType::Stop => 2,
            ProcessControlType::Continue => 3,
            ProcessControlType::Interrupt => 4,
            ProcessControlType::Quit => 5,
            ProcessControlType::Hangup => 6,
            ProcessControlType::ChildExit => 7,
            ProcessControlType::PipeBroken => 8,
            ProcessControlType::Alarm => 9,
            ProcessControlType::IoReady => 10,
            ProcessControlType::User(n) => {
                // Constrain user signals to 0-20 to avoid collisions
                // User signals beyond 20 are treated as 20
                11 + n.min(20)
            }
        };
        self.blocked_process_control &= !(1 << bit);
    }

    /// Check if a ProcessControlType is blocked
    pub fn is_process_control_blocked(&self, ptype: ProcessControlType) -> bool {
        if self.block_all {
            return true;
        }
        let bit = match ptype {
            ProcessControlType::Terminate => 0,
            ProcessControlType::Kill => 1,
            ProcessControlType::Stop => 2,
            ProcessControlType::Continue => 3,
            ProcessControlType::Interrupt => 4,
            ProcessControlType::Quit => 5,
            ProcessControlType::Hangup => 6,
            ProcessControlType::ChildExit => 7,
            ProcessControlType::PipeBroken => 8,
            ProcessControlType::Alarm => 9,
            ProcessControlType::IoReady => 10,
            ProcessControlType::User(n) => {
                // Constrain user signals to 0-20 to avoid collisions
                // User signals beyond 20 are treated as 20
                11 + n.min(20)
            }
        };
        (self.blocked_process_control & (1 << bit)) != 0
    }

    /// Check if an event content is blocked
    pub fn is_blocked(&self, content: &EventContent) -> bool {
        if self.block_all {
            return true;
        }
        match content {
            EventContent::ProcessControl(ptype) => self.is_process_control_blocked(*ptype),
            EventContent::Notification(ntype) => {
                let bit = *ntype as u64;
                (self.blocked_notifications & (1 << bit)) != 0
            }
            EventContent::Custom { namespace, .. } => {
                self.blocked_namespaces.iter().any(|ns| ns == namespace)
            }
            _ => false,
        }
    }
}

/// Scarlet Native ABI state
#[derive(Clone)]
pub struct ScarletAbi {
    /// TLS (Thread Local Storage) pointer for this task
    pub tls_pointer: Option<usize>,
    /// clear_child_tid pointer for thread exit notification (Linux-compatible)
    pub clear_child_tid_ptr: Option<usize>,
    /// Event handler table: EventContent discriminant -> handler entry
    pub event_handlers: BTreeMap<u8, EventHandlerEntry>,
    /// Default handler for unhandled events (None = ignore)
    pub default_event_handler: Option<EventHandlerEntry>,
    /// Event mask for blocking events
    pub event_mask: EventMask,
    /// Pending events that were blocked (stored for later delivery)
    pub pending_events: Vec<Event>,
}

impl Default for ScarletAbi {
    fn default() -> Self {
        Self {
            tls_pointer: None,
            clear_child_tid_ptr: None,
            event_handlers: BTreeMap::new(),
            default_event_handler: None,
            event_mask: EventMask::new(),
            pending_events: Vec::new(),
        }
    }
}

impl ScarletAbi {
    /// Get the TLS pointer for this task
    pub fn tls_pointer(&self) -> Option<usize> {
        self.tls_pointer
    }

    /// Set the TLS pointer for this task
    pub fn set_tls_pointer(&mut self, ptr: usize) {
        self.tls_pointer = Some(ptr);
    }

    /// Clear the TLS pointer for this task
    pub fn clear_tls_pointer(&mut self) {
        self.tls_pointer = None;
    }

    /// Set the clear_child_tid pointer for thread exit notification
    pub fn set_clear_child_tid(&mut self, ptr: usize) {
        self.clear_child_tid_ptr = Some(ptr);
    }

    /// Handle task exit with TLS cleanup (Linux-compatible)
    pub fn on_task_exit(&mut self, task: &crate::task::Task) {
        // Linux-compatible behavior: write 0 to clear_child_tid and futex wake
        if let Some(ptr) = self.clear_child_tid_ptr {
            if let Some(paddr) = task.vm_manager.translate_to_kva(ptr) {
                unsafe {
                    *(paddr as *mut i32) = 0;
                }
            }
            // Note: Futex wake for clear_child_tid is handled by the Linux ABI's
            // on_task_exit implementation. For Scarlet Native, we just clear the value.
        }
    }

    /// Register an event handler for a specific event content type
    pub fn register_event_handler(
        &mut self,
        content_type: u8,
        handler: EventHandler,
        synchronous: bool,
    ) {
        self.event_handlers.insert(
            content_type,
            EventHandlerEntry {
                handler,
                synchronous,
            },
        );
    }

    /// Unregister an event handler for a specific event content type
    pub fn unregister_event_handler(&mut self, content_type: u8) {
        self.event_handlers.remove(&content_type);
    }

    /// Set the default event handler for unhandled events
    pub fn set_default_event_handler(&mut self, handler: EventHandler, synchronous: bool) {
        self.default_event_handler = Some(EventHandlerEntry {
            handler,
            synchronous,
        });
    }

    /// Clear the default event handler
    pub fn clear_default_event_handler(&mut self) {
        self.default_event_handler = None;
    }

    /// Get the event handler for a specific event content type
    fn get_event_handler(&self, content: &EventContent) -> Option<EventHandlerEntry> {
        let content_type = content_type_discriminant(content);
        self.event_handlers
            .get(&content_type)
            .copied()
            .or(self.default_event_handler)
    }

    /// Handle an incoming event (called by EventManager)
    pub fn handle_incoming_event(
        &mut self,
        event: Event,
        task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        // Check if event is blocked by mask
        if self.event_mask.is_blocked(&event.content) {
            // Store in pending queue for later delivery when unblocked
            // Enforce maximum queue length to prevent unbounded memory growth
            if self.pending_events.len() >= MAX_PENDING_EVENTS {
                // Drop oldest event (FIFO overflow policy)
                self.pending_events.remove(0);
                crate::println!(
                    "[ScarletAbi] Warning: Pending event queue overflow, dropping oldest event"
                );
            }
            self.pending_events.push(event);
            return Ok(());
        }

        // Process the event immediately
        self.process_event(event, task)
    }

    /// Process a single event
    fn process_event(&self, event: Event, task: &crate::task::Task) -> Result<(), &'static str> {
        match &event.content {
            EventContent::ProcessControl(ptype) => self.handle_process_control_event(*ptype, task),
            EventContent::Message { .. } => {
                // Message events require a handler
                if let Some(handler) = self.get_event_handler(&event.content) {
                    self.invoke_user_handler(handler, event, task)
                } else {
                    Ok(()) // No handler, ignore
                }
            }
            EventContent::Notification(ntype) => self.handle_notification_event(*ntype, task),
            EventContent::Custom { .. } => {
                if let Some(handler) = self.get_event_handler(&event.content) {
                    self.invoke_user_handler(handler, event, task)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Handle process control events
    fn handle_process_control_event(
        &self,
        ptype: ProcessControlType,
        task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        match ptype {
            ProcessControlType::Terminate | ProcessControlType::Kill => {
                // Exit the task with appropriate status
                let exit_code = match ptype {
                    ProcessControlType::Kill => 128 + 9,       // SIGKILL-like
                    ProcessControlType::Terminate => 128 + 15, // SIGTERM-like
                    _ => 1,
                };
                task.exit(exit_code);
                Ok(())
            }
            ProcessControlType::Stop => {
                task.set_state(crate::task::TaskState::Blocked(
                    crate::task::BlockedType::Interruptible,
                ));
                crate::sched::scheduler::mark_blocked(task.get_id());
                crate::sched::scheduler::remove_from_ready_queues(task.get_id());
                Ok(())
            }
            ProcessControlType::Continue => {
                let current_state = task.get_state();
                if matches!(current_state, crate::task::TaskState::Blocked(_)) {
                    task.set_state(crate::task::TaskState::Ready);
                    crate::sched::scheduler::unmark_blocked(task.get_id());
                    crate::sched::scheduler::push_ready_task(
                        crate::arch::get_cpu().get_cpuid(),
                        task.get_id(),
                    );
                }
                Ok(())
            }
            ProcessControlType::Interrupt => {
                // Call handler if registered, otherwise default action
                if let Some(handler) = self.get_event_handler(&EventContent::ProcessControl(ptype))
                {
                    self.invoke_user_handler(
                        handler,
                        Event::direct_process_control(
                            task.get_id() as u32,
                            ptype,
                            EventPriority::High,
                            true,
                        ),
                        task,
                    )
                } else {
                    // Default: terminate with SIGINT-like exit code
                    task.exit(128 + 2);
                    Ok(())
                }
            }
            _ => {
                // Other control types: call handler if registered
                if let Some(handler) = self.get_event_handler(&EventContent::ProcessControl(ptype))
                {
                    self.invoke_user_handler(
                        handler,
                        Event::direct_process_control(
                            task.get_id() as u32,
                            ptype,
                            EventPriority::Normal,
                            false,
                        ),
                        task,
                    )
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Handle notification events
    fn handle_notification_event(
        &self,
        ntype: crate::ipc::event::NotificationType,
        task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        // Check if there's a handler registered
        if let Some(handler) = self.get_event_handler(&EventContent::Notification(ntype)) {
            self.invoke_user_handler(
                handler,
                Event::notification_to_task(task.get_id() as u32, ntype),
                task,
            )
        } else {
            // Default handling for specific notifications
            match ntype {
                crate::ipc::event::NotificationType::TaskCompleted => {
                    // Wake up parent if waiting
                    if let Some(parent_id) = task.get_parent_id() {
                        crate::task::wake_task_waiters(task.get_id());
                        crate::task::wake_parent_waiters(parent_id);
                    }
                }
                _ => {}
            }
            Ok(())
        }
    }

    /// Invoke a user-space event handler
    ///
    /// Sets up the user stack with a signal frame that preserves the interrupted
    /// context, then modifies the trapframe to jump to the registered handler.
    ///
    /// # Signal frame layout on user stack (high to low):
    ///
    /// ```text
    /// +--------------------------+  <- original SP
    /// |   saved sp (8 bytes)     |
    /// |   saved elr (8 bytes)    |
    /// |   saved regs[0..30]      |  (31 × 8 = 248 bytes)
    /// |   event content type (8) |
    /// |   event subtype    (8)   |
    /// |   trampoline code  (8)   |  <- svc for syscall 643 (event_return)
    /// +--------------------------+  <- new SP (16-byte aligned)
    /// ```
    ///
    /// # Arguments passed to handler
    /// - x0: event content type discriminant
    /// - x1: event subtype
    /// - x2: pointer to saved context on stack
    fn invoke_user_handler(
        &self,
        handler: EventHandlerEntry,
        event: Event,
        task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        let trapframe = task.get_trapframe();

        let (content_type, subtype) = match &event.content {
            EventContent::ProcessControl(ptype) => {
                let sub = match ptype {
                    ProcessControlType::Terminate => 0,
                    ProcessControlType::Kill => 1,
                    ProcessControlType::Stop => 2,
                    ProcessControlType::Continue => 3,
                    ProcessControlType::Interrupt => 4,
                    ProcessControlType::Quit => 5,
                    ProcessControlType::Hangup => 6,
                    ProcessControlType::ChildExit => 7,
                    ProcessControlType::PipeBroken => 8,
                    ProcessControlType::Alarm => 9,
                    ProcessControlType::IoReady => 10,
                    ProcessControlType::User(id) => 256 + *id as usize,
                };
                (0usize, sub)
            }
            EventContent::Message { .. } => (1usize, 0usize),
            EventContent::Notification(ntype) => (2usize, *ntype as usize),
            EventContent::Custom { event_id, .. } => (3usize, *event_id as usize),
        };

        let mut sp = trapframe.sp as usize;

        // trampoline(8) + subtype(8) + content_type(8) + regs(31×8) + elr(8) + sp(8) = 288
        const SIGNAL_FRAME_SIZE: usize = 8 + 8 + 8 + (31 * 8) + 8 + 8;
        sp -= SIGNAL_FRAME_SIZE;
        sp &= !0xF;

        let frame_base = sp;

        // Trampoline: movz x8, #643 (0xd2805068) + svc #0 (0xd4000001)
        let trampoline_instr_0: u32 = 0xd2805068;
        let trampoline_instr_1: u32 = 0xd4000001;

        unsafe {
            let paddr = task
                .vm_manager
                .translate_to_kva(frame_base)
                .ok_or("Failed to translate signal frame address")?;
            *(paddr as *mut u32) = trampoline_instr_0;
            *((paddr as *mut u32).add(1)) = trampoline_instr_1;

            let paddr = task
                .vm_manager
                .translate_to_kva(frame_base + 8)
                .ok_or("Failed to translate signal frame address")?;
            *(paddr as *mut usize) = subtype;

            let paddr = task
                .vm_manager
                .translate_to_kva(frame_base + 16)
                .ok_or("Failed to translate signal frame address")?;
            *(paddr as *mut usize) = content_type;

            for i in 0..31 {
                let paddr = task
                    .vm_manager
                    .translate_to_kva(frame_base + 24 + i * 8)
                    .ok_or("Failed to translate signal frame address")?;
                *(paddr as *mut usize) = trapframe.regs.reg[i];
            }

            // saved elr at frame_base + 24 + 248 = frame_base + 272
            let paddr = task
                .vm_manager
                .translate_to_kva(frame_base + 272)
                .ok_or("Failed to translate signal frame address")?;
            *(paddr as *mut u64) = trapframe.elr;

            // saved sp at frame_base + 280
            let paddr = task
                .vm_manager
                .translate_to_kva(frame_base + 280)
                .ok_or("Failed to translate signal frame address")?;
            *(paddr as *mut u64) = trapframe.sp;
        }

        trapframe.elr = handler.handler as u64;
        trapframe.sp = sp as u64;
        trapframe.regs.reg[0] = content_type;
        trapframe.regs.reg[1] = subtype;
        trapframe.regs.reg[2] = frame_base + 24;
        trapframe.regs.reg[30] = frame_base; // LR = trampoline address

        Ok(())
    }

    /// Restore context saved by `invoke_user_handler` (syscall 643 — event_return).
    ///
    /// # Signal frame layout (AArch64)
    /// ```text
    ///   [sp + 0]:   trampoline code  (8 bytes)
    ///   [sp + 8]:   event subtype    (8 bytes)
    ///   [sp + 16]:  content type     (8 bytes)
    ///   [sp + 24]:  saved regs[0..30] (248 bytes)
    ///   [sp + 272]: saved elr        (8 bytes)
    ///   [sp + 280]: saved sp         (8 bytes)
    /// ```
    pub fn event_return(
        trapframe: &mut crate::arch::Trapframe,
        task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        let frame_base = trapframe.sp as usize; // SP points to signal frame

        unsafe {
            for i in 0..31 {
                let paddr = task
                    .vm_manager
                    .translate_to_kva(frame_base + 24 + i * 8)
                    .ok_or("Failed to translate signal frame address")?;
                trapframe.regs.reg[i] = *(paddr as *const usize);
            }

            let paddr = task
                .vm_manager
                .translate_to_kva(frame_base + 272)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.elr = *(paddr as *const u64);

            let paddr = task
                .vm_manager
                .translate_to_kva(frame_base + 280)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.sp = *(paddr as *const u64);
        }

        Ok(())
    }

    /// Process any pending events (called when mask changes)
    pub fn process_pending_events(&mut self, task: &crate::task::Task) -> Result<(), &'static str> {
        // We must not drop events that are still blocked; they should remain pending
        // until the event mask allows them to be delivered.

        // First, separate events into blocked and unblocked
        let mut still_blocked: Vec<Event> = Vec::new();
        let mut events_to_process: Vec<Event> = Vec::new();

        for event in self.pending_events.drain(..) {
            if self.event_mask.is_blocked(&event.content) {
                // Keep blocked events pending for future processing
                still_blocked.push(event);
            } else {
                // Queue for processing
                events_to_process.push(event);
            }
        }

        // Restore the blocked events as the new pending queue
        self.pending_events = still_blocked;

        // Now process unblocked events
        for event in events_to_process {
            self.process_event(event, task)?;
        }

        Ok(())
    }
}

/// Get the discriminant value for an EventContent variant
fn content_type_discriminant(content: &EventContent) -> u8 {
    match content {
        EventContent::ProcessControl(_) => 0,
        EventContent::Message { .. } => 1,
        EventContent::Notification(_) => 2,
        EventContent::Custom { .. } => 3,
    }
}

impl AbiModule for ScarletAbi {
    fn name() -> &'static str {
        "scarlet"
    }

    fn get_name(&self) -> alloc::string::String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone()) // ScarletAbi is Copy, so we can dereference and copy
    }

    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        syscall_handler(trapframe)
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
        current_abi: Option<&(dyn crate::abi::AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        // Stage 1: Basic format validation
        let magic_score = match file_object.as_file() {
            Some(file_obj) => {
                // Check ELF magic bytes (0x7F, 'E', 'L', 'F')
                let mut magic_buffer = [0u8; 4];
                file_obj.seek(SeekFrom::Start(0)).ok(); // Reset to start
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        if magic_buffer == [0x7F, b'E', b'L', b'F'] {
                            30 // Basic ELF format compatibility
                        } else {
                            return None; // Not an ELF file, cannot execute
                        }
                    }
                    _ => return None, // Read failed, cannot determine
                }
            }
            None => return None, // Not a file object
        };

        let mut confidence = magic_score;

        // Stage 2: ELF header checks
        if let Some(file_obj) = file_object.as_file() {
            // Check ELF header for Scarlet-specific OSABI (83)
            let mut osabi_buffer = [0u8; 1];
            file_obj.seek(SeekFrom::Start(7)).ok(); // OSABI is at
            match file_obj.read(&mut osabi_buffer) {
                Ok(bytes_read) if bytes_read == 1 => {
                    if osabi_buffer[0] == 83 {
                        // Scarlet OSABI
                        confidence += 70; // Strong indicator for Scarlet ABI
                    }
                }
                _ => return None, // Read failed, cannot determine
            }
        } else {
            return None; // Not a file object
        }

        // Stage 3: File path hints
        if file_path.ends_with(".elf") || file_path.contains("scarlet") {
            confidence += 15; // Scarlet-specific path indicators
        }

        // Stage 4: ABI inheritance bonus - high priority for same ABI
        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence += 40; // Strong inheritance bonus for Scarlet Native
            }
        }

        Some(confidence.min(100))
    }

    fn get_runtime_config(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
    ) -> Option<crate::abi::RuntimeConfig> {
        // Example: Delegate WebAssembly binaries to a Scarlet-native Wasm runtime
        // This demonstrates how to configure runtime delegation

        // Check for Wasm magic bytes (0x00 0x61 0x73 0x6D) or .wasm extension
        let is_wasm = if let Some(file_obj) = file_object.as_file() {
            let mut magic_buffer = [0u8; 4];
            // Save current position to restore later
            let original_pos = file_obj.seek(SeekFrom::Current(0)).ok();

            // Check magic bytes
            let has_wasm_magic = if file_obj.seek(SeekFrom::Start(0)).is_ok() {
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        magic_buffer == [0x00, 0x61, 0x73, 0x6D] // Wasm magic "\0asm"
                    }
                    _ => false,
                }
            } else {
                false
            };

            // Restore original file position
            if let Some(pos) = original_pos {
                let _ = file_obj.seek(SeekFrom::Start(pos));
            }

            has_wasm_magic
        } else {
            false
        } || file_path.ends_with(".wasm");

        if is_wasm {
            // Delegate to Scarlet-native Wasm runtime
            Some(crate::abi::RuntimeConfig {
                runtime_path: "/system/scarlet/bin/wasm-runtime".to_string(),
                runtime_abi: None, // Auto-detect (will be Scarlet native)
                runtime_args: alloc::vec!["--wasm".to_string()],
            })
        } else {
            // Not a Wasm binary, execute directly (or return None for unknown formats)
            None
        }
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &crate::task::Task,
        trapframe: &mut Trapframe,
    ) -> Result<(), &'static str> {
        // Get file object from KernelObject::File
        match file_object.as_file() {
            Some(file_obj) => {
                task.text_size.store(0, Ordering::SeqCst);
                task.data_size.store(0, Ordering::SeqCst);
                task.stack_size.store(0, Ordering::SeqCst);
                task.brk
                    .store(usize::MAX, core::sync::atomic::Ordering::SeqCst);

                // Create Scarlet-specific loading strategy
                let strategy = LoadStrategy {
                    choose_base_address: |target, needs_relocation| match (target, needs_relocation)
                    {
                        (LoadTarget::MainProgram, false) => 0, // ET_EXEC: absolute
                        (LoadTarget::MainProgram, true) => 0x10000, // ET_DYN: PIE
                        (LoadTarget::Interpreter, _) => 0x40000000, // Dynamic linker
                        (LoadTarget::SharedLib, _) => 0x50000000, // Shared libraries
                    },
                    resolve_interpreter: |requested| {
                        // Scarlet ABI: use interpreter as specified in ELF
                        requested.map(|s| s.to_string())
                    },
                };

                // Load and analyze the ELF file with Scarlet strategy
                match analyze_and_load_elf_with_strategy(file_obj, task, &strategy) {
                    Ok(elf_result) => {
                        // Set the name from argv[0] or use default
                        *task.name.write() = argv
                            .get(0)
                            .map_or("Unnamed Task".to_string(), |s| s.to_string());

                        // Clear old page table entries
                        let root_page_table =
                            vm::get_root_pagetable(task.vm_manager.get_asid()).unwrap();
                        root_page_table.unmap_all();

                        // Setup the new memory environment
                        vm::setup_trampoline_for_user(&task.vm_manager);
                        let stack_pointer = setup_user_stack(task).1;

                        // Handle different execution modes
                        match elf_result.mode {
                            ExecutionMode::Static => {
                                // Static linking - direct execution
                                task.set_entry_point(elf_result.entry_point as usize);
                            }
                            ExecutionMode::Dynamic {
                                ref interpreter_path,
                            } => {
                                // Dynamic linking - setup auxiliary vector and jump to interpreter
                                crate::println!(
                                    "Scarlet ABI: Using dynamic linker at {}",
                                    interpreter_path
                                );

                                // Build auxiliary vector for dynamic linking
                                let auxv = build_auxiliary_vector(&elf_result);

                                // Setup auxiliary vector on stack
                                match setup_auxiliary_vector_on_stack(task, &auxv) {
                                    Ok(_auxv_addr) => {
                                        crate::println!(
                                            "Scarlet ABI: Auxiliary vector setup complete"
                                        );
                                    }
                                    Err(e) => {
                                        crate::println!(
                                            "Scarlet ABI: Failed to setup auxiliary vector: {}",
                                            e.message
                                        );
                                        return Err("Failed to setup auxiliary vector");
                                    }
                                }

                                task.set_entry_point(elf_result.entry_point as usize);
                            }
                        }

                        // Reset task's registers for clean start
                        task.vcpu.lock().reset_iregs();
                        task.vcpu.lock().set_sp(stack_pointer);

                        // Setup argv/envp on stack following Unix and AArch64 conventions
                        let (adjusted_sp, argv_ptr) =
                            self.setup_arguments_on_stack(task, argv, envp, stack_pointer)?;
                        task.vcpu.lock().set_sp(adjusted_sp);

                        // Set AArch64 calling convention registers
                        // x0 (reg[0]) = argc
                        // x1 (reg[1]) = argv pointer
                        task.vcpu.lock().iregs.reg[0] = argv.len(); // argc
                        task.vcpu.lock().iregs.reg[1] = argv_ptr; // argv array pointer

                        // Switch to the new task
                        task.vcpu.lock().switch(trapframe);
                        Ok(())
                    }
                    Err(e) => {
                        // Log error details
                        crate::println!("ELF loading failed: {}", e.message);
                        Err("Failed to load ELF binary")
                    }
                }
            }
            None => Err("Invalid file object type for binary execution"),
        }
    }

    fn choose_load_address(
        &self,
        elf_type: u16,
        target: crate::task::elf_loader::LoadTarget,
    ) -> Option<u64> {
        use crate::task::elf_loader::{ET_DYN, LoadTarget};

        // Scarlet Native ABI uses standard Linux-style memory layout
        if elf_type == ET_DYN {
            match target {
                LoadTarget::MainProgram => {
                    // PIE main program: low memory area, avoiding null pointer region
                    Some(0x10000) // 64KB base
                }
                LoadTarget::Interpreter => {
                    // Dynamic linker: high memory area to avoid conflicts with main program
                    Some(0x40000000) // 1GB base
                }
                LoadTarget::SharedLib => {
                    // Shared libraries: medium memory area
                    Some(0x50000000) // 1.25GB base
                }
            }
        } else {
            None // Use kernel default for ET_EXEC and other types
        }
    }

    fn normalize_env_to_scarlet(&self, envp: &mut Vec<String>) {
        // Scarlet ABI is already in canonical format, but ensure all paths are absolute
        // Modify in-place to avoid allocations

        for env_var in envp.iter_mut() {
            if let Some(eq_pos) = env_var.find('=') {
                let key = &env_var[..eq_pos];
                let value = &env_var[eq_pos + 1..];

                let normalized_value = match key {
                    "PATH" | "LD_LIBRARY_PATH" => {
                        // Ensure all paths are in absolute Scarlet namespace format
                        self.normalize_path_to_absolute_scarlet(value)
                    }
                    "HOME" => {
                        // Ensure home directory is absolute
                        if value.starts_with('/') {
                            value.to_string()
                        } else {
                            format!("/home/{}", value)
                        }
                    }
                    _ => value.to_string(), // Most variables pass through unchanged
                };

                // Update in-place if value changed
                let new_env_var = format!("{}={}", key, normalized_value);
                if new_env_var != *env_var {
                    *env_var = new_env_var;
                }
            }
        }
    }

    fn denormalize_env_from_scarlet(&self, envp: &mut Vec<String>) {
        // For Scarlet ABI, canonical format is the native format
        // But ensure proper Scarlet-specific defaults exist

        // Convert to temporary map for easier processing
        let mut env_map = BTreeMap::new();
        for env_var in envp.iter() {
            if let Some(eq_pos) = env_var.find('=') {
                let key = env_var[..eq_pos].to_string();
                let value = env_var[eq_pos + 1..].to_string();
                env_map.insert(key, value);
            }
        }

        // Add defaults if they don't exist
        if !env_map.contains_key("PATH") {
            env_map.insert(
                "PATH".to_string(),
                "/system/scarlet/bin:/bin:/usr/bin".to_string(),
            );
        }

        if !env_map.contains_key("SHELL") {
            env_map.insert("SHELL".to_string(), "/system/scarlet/bin/sh".to_string());
        }

        // Convert back to Vec<String> format
        envp.clear();
        for (key, value) in env_map.iter() {
            envp.push(format!("{}={}", key, value));
        }
    }

    fn setup_overlay_environment(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
        system_path: &str,
        config_path: &str,
    ) -> Result<(), &'static str> {
        // Scarlet ABI uses overlay mount with system Scarlet tools and config persistence
        let lower_vfs_list = alloc::vec![(base_vfs, system_path)];
        let upper_vfs = base_vfs;
        let fs = match OverlayFS::new_from_paths_and_vfs(
            Some((upper_vfs, config_path)),
            lower_vfs_list,
            "/",
        ) {
            Ok(fs) => fs,
            Err(e) => {
                crate::println!(
                    "Failed to create overlay filesystem for Scarlet ABI: {}",
                    e.message
                );
                return Err("Failed to create Scarlet overlay environment");
            }
        };

        match target_vfs.mount(fs, "/", 0) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!(
                    "Failed to create cross-VFS overlay for Scarlet ABI: {}",
                    e.message
                );
                Err("Failed to create Scarlet overlay environment")
            }
        }
    }

    fn setup_shared_resources(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
    ) -> Result<(), &'static str> {
        // Scarlet shared resource setup: bind mount common directories and Scarlet gateway
        match create_dir_if_not_exists(target_vfs, "/home") {
            Ok(()) => {}
            Err(e) => {
                crate::println!(
                    "Failed to create /home directory for Scarlet: {}",
                    e.message
                );
                return Err("Failed to create /home directory for Scarlet");
            }
        }

        match target_vfs.bind_mount_from(base_vfs, "/home", "/home") {
            Ok(()) => {}
            Err(_e) => {}
        }

        match create_dir_if_not_exists(target_vfs, "/data") {
            Ok(()) => {}
            Err(e) => {
                crate::println!(
                    "Failed to create /data directory for Scarlet: {}",
                    e.message
                );
                return Err("Failed to create /data directory for Scarlet");
            }
        }

        match target_vfs.bind_mount_from(base_vfs, "/data/shared", "/data/shared") {
            Ok(()) => {}
            Err(_e) => {}
        }

        // Bind mount /dev for device access
        match create_dir_if_not_exists(target_vfs, "/dev") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to create /dev directory for Scarlet: {}", e.message);
                return Err("Failed to create /dev directory for Scarlet");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/dev", "/dev") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to bind mount /dev for Scarlet: {}", e.message);
                return Err("Failed to bind mount /dev for Scarlet");
            }
        }

        // Bind moutt /tmp for temporary files
        match create_dir_if_not_exists(target_vfs, "/tmp") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to create /tmp directory for Scarlet: {}", e.message);
                return Err("Failed to create /tmp directory for Scarlet");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/tmp", "/tmp") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to bind mount /tmp for Scarlet: {}", e.message);
                return Err("Failed to bind mount /tmp for Scarlet");
            }
        }

        // Setup gateway to native Scarlet environment (read-only for security)
        match create_dir_if_not_exists(target_vfs, "/scarlet") {
            Ok(()) => {}
            Err(e) => {
                crate::println!(
                    "Failed to create /scarlet directory for Scarlet: {}",
                    e.message
                );
                return Err("Failed to create /scarlet directory for Scarlet");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/", "/scarlet") {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!(
                    "Failed to bind mount native Scarlet root to /scarlet for Scarlet: {}",
                    e.message
                );
                return Err("Failed to bind mount native Scarlet root to /scarlet for Scarlet");
            }
        }
    }

    fn on_task_exit(&mut self, task: &crate::task::Task) {
        // Delegate to the implementation method
        self.on_task_exit(task);
    }

    fn set_tls_pointer(&mut self, ptr: usize) {
        self.tls_pointer = Some(ptr);
    }

    fn get_tls_pointer(&self) -> Option<usize> {
        self.tls_pointer
    }

    fn set_clear_child_tid(&mut self, ptr: usize) {
        self.clear_child_tid_ptr = Some(ptr);
    }

    fn handle_event(
        &mut self,
        event: crate::ipc::Event,
        _target_task_id: u32,
    ) -> Result<(), &'static str> {
        // Get the current task to process the event
        if let Some(task) = crate::task::mytask() {
            self.handle_incoming_event(event, task)
        } else {
            Err("No current task to handle event")
        }
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

impl ScarletAbi {
    /// Setup argc, argv, and envp on the user stack following Unix conventions
    ///
    /// Standard Unix stack layout (from high to low addresses):
    /// ```
    /// [high addresses]
    /// envp strings (null-terminated)
    /// argv strings (null-terminated)
    /// envp[] array (null-terminated pointer array)
    /// argv[] array (null-terminated pointer array)
    /// argc (integer)
    /// [low addresses - returned stack pointer]
    /// ```
    ///
    /// # Arguments
    /// * `task` - The task to set up arguments for
    /// * `argv` - Command line arguments
    /// * `envp` - Environment variables
    /// * `initial_sp` - Initial stack pointer from setup_user_stack
    ///
    /// # Returns
    /// Tuple of (new stack pointer, argv array pointer)
    fn setup_arguments_on_stack(
        &self,
        task: &crate::task::Task,
        argv: &[&str],
        envp: &[&str],
        initial_sp: usize,
    ) -> Result<(usize, usize), &'static str> {
        // Calculate total size needed
        let argc = argv.len();
        let envc = envp.len();

        // Calculate string sizes (including null terminators)
        let argv_strings_size: usize = argv.iter().map(|s| s.len() + 1).sum();
        let envp_strings_size: usize = envp.iter().map(|s| s.len() + 1).sum();

        // Calculate pointer array sizes (including null terminators)
        let argv_array_size = (argc + 1) * core::mem::size_of::<usize>(); // +1 for NULL terminator
        let envp_array_size = (envc + 1) * core::mem::size_of::<usize>(); // +1 for NULL terminator
        let argc_size = core::mem::size_of::<usize>();

        // Total space needed
        let total_size =
            argc_size + argv_array_size + envp_array_size + argv_strings_size + envp_strings_size;

        // Align to 16-byte boundary for ABI compliance
        let aligned_total_size = (total_size + 15) & !15;

        // Calculate new stack pointer
        let new_sp = initial_sp - aligned_total_size;

        // Layout from new_sp (low) to initial_sp (high):
        // argc | argv[] | envp[] | argv_strings | envp_strings

        let mut current_addr = new_sp;

        // 1. Write argc
        self.write_to_stack_memory(task, current_addr, &argc.to_le_bytes())?;
        current_addr += argc_size;

        // 2. Save argv array pointer for return value
        let argv_ptr = current_addr;

        // 3. Calculate string positions first
        let argv_strings_start = current_addr + argv_array_size + envp_array_size;
        let envp_strings_start = argv_strings_start + argv_strings_size;

        // 4. Write argv[] array
        let mut string_addr = argv_strings_start;
        for i in 0..argc {
            self.write_to_stack_memory(task, current_addr, &string_addr.to_le_bytes())?;
            current_addr += core::mem::size_of::<usize>();
            string_addr += argv[i].len() + 1; // Move to next string position
        }
        // NULL terminate argv[]
        let null_ptr: usize = 0;
        self.write_to_stack_memory(task, current_addr, &null_ptr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();

        // 5. Write envp[] array
        string_addr = envp_strings_start;
        for i in 0..envc {
            self.write_to_stack_memory(task, current_addr, &string_addr.to_le_bytes())?;
            current_addr += core::mem::size_of::<usize>();
            string_addr += envp[i].len() + 1; // Move to next string position
        }
        // NULL terminate envp[]
        self.write_to_stack_memory(task, current_addr, &null_ptr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();

        // 6. Write argv strings
        for arg in argv {
            self.write_string_to_stack(task, current_addr, arg)?;
            current_addr += arg.len() + 1; // +1 for null terminator
        }

        // 7. Write envp strings
        for env in envp {
            self.write_string_to_stack(task, current_addr, env)?;
            current_addr += env.len() + 1; // +1 for null terminator
        }

        Ok((new_sp, argv_ptr))
    }

    /// Write bytes to stack memory using virtual memory translation
    fn write_to_stack_memory(
        &self,
        task: &crate::task::Task,
        vaddr: usize,
        data: &[u8],
    ) -> Result<(), &'static str> {
        let mut written = 0usize;
        while written < data.len() {
            let current_vaddr = vaddr + written;
            let page_off = current_vaddr & (crate::environment::PAGE_SIZE - 1);
            let chunk_len = core::cmp::min(
                data.len() - written,
                crate::environment::PAGE_SIZE - page_off,
            );

            match task.vm_manager.translate_to_kva(current_vaddr) {
                Some(paddr) => {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            data[written..written + chunk_len].as_ptr(),
                            paddr as *mut u8,
                            chunk_len,
                        );
                    }
                    written += chunk_len;
                }
                None => return Err("Failed to translate virtual address for stack write"),
            }
        }

        Ok(())
    }

    /// Write a null-terminated string to stack memory
    fn write_string_to_stack(
        &self,
        task: &crate::task::Task,
        vaddr: usize,
        string: &str,
    ) -> Result<(), &'static str> {
        // Write the string content
        self.write_to_stack_memory(task, vaddr, string.as_bytes())?;
        // Write null terminator
        self.write_to_stack_memory(task, vaddr + string.len(), &[0u8])?;
        Ok(())
    }

    /// Normalize path string to absolute Scarlet namespace format
    ///
    /// This ensures all paths in PATH-like variables are absolute and
    /// in the proper Scarlet namespace format.
    fn normalize_path_to_absolute_scarlet(&self, path_value: &str) -> String {
        let paths: Vec<&str> = path_value.split(':').collect();
        let mut normalized_paths = Vec::new();

        for path in paths {
            if path.starts_with('/') {
                // Already absolute - ensure it's in proper Scarlet namespace
                if path.starts_with("/system/scarlet/") || path.starts_with("/scarlet/") {
                    normalized_paths.push(path.to_string());
                } else {
                    // Map standard paths to Scarlet namespace
                    let mapped_path = match path {
                        "/bin" => "/system/scarlet/bin",
                        "/usr/bin" => "/system/scarlet/usr/bin",
                        "/usr/local/bin" => "/system/scarlet/usr/local/bin",
                        "/sbin" => "/system/scarlet/sbin",
                        "/usr/sbin" => "/system/scarlet/usr/sbin",
                        "/lib" => "/system/scarlet/lib",
                        "/usr/lib" => "/system/scarlet/usr/lib",
                        "/usr/local/lib" => "/system/scarlet/usr/local/lib",
                        _ => path, // Keep other absolute paths as-is
                    };
                    normalized_paths.push(mapped_path.to_string());
                }
            } else if !path.is_empty() {
                // Relative paths - prefix with current working directory or make absolute
                normalized_paths.push(format!("/{}", path));
            }
            // Skip empty paths
        }

        normalized_paths.join(":")
    }
}

fn create_dir_if_not_exists(vfs: &Arc<VfsManager>, path: &str) -> Result<(), FileSystemError> {
    match vfs.create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if e.kind == FileSystemErrorKind::AlreadyExists {
                Ok(()) // Directory already exists, nothing to do
            } else {
                Err(e) // Some other error occurred
            }
        }
    }
}

fn register_scarlet_abi() {
    register_abi!(ScarletAbi);
}

late_initcall!(register_scarlet_abi);
