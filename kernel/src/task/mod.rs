//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

pub mod elf_loader;
pub mod syscall;

extern crate alloc;

use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use spin::Mutex;

use crate::abi::{AbiModule, scarlet::ScarletAbi};
use crate::sync::waker::Waker;
use crate::{
    arch::{
        KernelContext, Trapframe, get_cpu, trap::user::arch_switch_to_user_space, vcpu::Vcpu,
        vm::alloc_virtual_address_space,
    },
    environment::{
        DEAFAULT_MAX_TASK_DATA_SIZE, DEAFAULT_MAX_TASK_STACK_SIZE, DEAFAULT_MAX_TASK_TEXT_SIZE,
        KERNEL_VM_STACK_END, PAGE_SIZE, USER_STACK_END,
    },
    fs::VfsManager,
    ipc::{EventContent, event::ProcessControlType},
    mem::page::{Page, allocate_page, free_boxed_page},
    object::handle::HandleTable,
    sched::scheduler::{Scheduler, get_scheduler},
    timer::{TimerHandler, add_timer, get_tick},
    vm::{
        manager::VirtualMemoryManager,
        map_task_kernel_stack_window, user_kernel_vm_init, user_vm_init,
        vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryRegion},
    },
};
use alloc::collections::BTreeMap;
use core::ops::Range;
use spin::Once;

/// Global registry of task-specific wakers for waitpid
static WAITPID_WAKERS: Once<Mutex<BTreeMap<usize, Waker>>> = Once::new();

/// Global registry of parent task wakers for waitpid(-1) operations
/// Each parent task has a waker that gets triggered when any of its children exit
static PARENT_WAITPID_WAKERS: Once<Mutex<BTreeMap<usize, Waker>>> = Once::new();

/// Initialize the waitpid wakers registry
fn init_waitpid_wakers() -> Mutex<BTreeMap<usize, Waker>> {
    Mutex::new(BTreeMap::new())
}

/// Initialize the parent waitpid waker registry
fn init_parent_waitpid_wakers() -> Mutex<BTreeMap<usize, Waker>> {
    Mutex::new(BTreeMap::new())
}

/// Get or create a waker for waitpid/wait operations for a specific task
///
/// This function returns a reference to the waker associated with the given task ID,
/// used exclusively for waitpid/wait (child termination wait) synchronization.
/// If no waker exists for the task, a new one is created.
///
/// # Arguments
///
/// * `task_id` - The ID of the task to get a waitpid/wait waker for
///
/// # Returns
///
/// A reference to the waker for the specified task
pub fn get_waitpid_waker(task_id: usize) -> &'static Waker {
    let wakers_mutex = WAITPID_WAKERS.call_once(init_waitpid_wakers);
    let mut wakers = wakers_mutex.lock();
    if !wakers.contains_key(&task_id) {
        let waker_name = alloc::format!("task_{}", task_id);
        // We need to create a static string for the waker name
        let static_name = Box::leak(waker_name.into_boxed_str());
        wakers.insert(task_id, Waker::new_interruptible(static_name));
    }
    // This is safe because we know the waker exists and won't be removed
    // until the task is cleaned up
    unsafe {
        let waker_ptr = wakers.get(&task_id).unwrap() as *const Waker;
        &*waker_ptr
    }
}

// pub fn get_select_waker(...) was removed; use object-level Selectable::wait_until_ready

/// Get or create a parent waker for waitpid(-1) operations
///
/// This waker is used when a parent process calls waitpid(-1) to wait for any child to exit.
/// It is separate from the task-specific waitpid wakers to avoid conflicts, and is used
/// exclusively for waitpid(-1) (any child termination wait) synchronization.
///
/// # Arguments
///
/// * `parent_id` - The ID of the parent task
///
/// # Returns
///
/// A reference to the parent waker
pub fn get_parent_waitpid_waker(parent_id: usize) -> &'static Waker {
    let wakers_mutex = PARENT_WAITPID_WAKERS.call_once(init_parent_waitpid_wakers);
    let mut wakers = wakers_mutex.lock();

    // Create a new waker if it doesn't exist
    if !wakers.contains_key(&parent_id) {
        let waker_name = alloc::format!("parent_waker_{}", parent_id);
        // We need to leak the string to make it 'static
        let static_name = alloc::boxed::Box::leak(waker_name.into_boxed_str());
        wakers.insert(parent_id, Waker::new_interruptible(static_name));
    }

    // Return a reference to the waker
    // This is safe because the BTreeMap is never dropped and the Waker is never moved
    unsafe {
        let waker_ptr = wakers.get(&parent_id).unwrap() as *const Waker;
        &*waker_ptr
    }
}

/// Wake up any processes waiting for a specific task
///
/// This function should be called when a task exits to wake up
/// any parent processes that are waiting for this specific task.
///
/// # Arguments
///
/// * `task_id` - The ID of the task that has exited
pub fn wake_task_waiters(task_id: usize) {
    let wakers_mutex = WAITPID_WAKERS.call_once(init_waitpid_wakers);
    let wakers = wakers_mutex.lock();
    if let Some(waker) = wakers.get(&task_id) {
        waker.wake_all();
    }
}

/// Wake up a parent process waiting for any child (waitpid(-1))
///
/// This function should be called when any child of a parent exits.
///
/// # Arguments
///
/// * `parent_id` - The ID of the parent task
pub fn wake_parent_waiters(parent_id: usize) {
    let wakers_mutex = PARENT_WAITPID_WAKERS.call_once(init_parent_waitpid_wakers);
    let wakers = wakers_mutex.lock();
    if let Some(waker) = wakers.get(&parent_id) {
        waker.wake_all();
    }
}

/// Clean up the waker for a specific task
///
/// This function should be called when a task is completely cleaned up
/// to remove its waker from the global registry.
///
/// # Arguments
///
/// * `task_id` - The ID of the task to clean up
pub fn cleanup_task_waker(task_id: usize) {
    let wakers_mutex = WAITPID_WAKERS.call_once(init_waitpid_wakers);
    let mut wakers = wakers_mutex.lock();
    wakers.remove(&task_id);
}

/// Clean up the parent waker for a specific task
///
/// This function should be called when a parent task is completely cleaned up.
///
/// # Arguments
///
/// * `parent_id` - The ID of the parent task to clean up
pub fn cleanup_parent_waker(parent_id: usize) {
    let wakers_mutex = PARENT_WAITPID_WAKERS.call_once(init_parent_waitpid_wakers);
    let mut wakers = wakers_mutex.lock();
    wakers.remove(&parent_id);
}

// pub fn cleanup_select_waker(...) was removed along with task-level select waker

/// Types of blocked states for tasks
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BlockedType {
    /// Interruptible blocking - can be interrupted by signals
    Interruptible,
    /// Uninterruptible blocking - cannot be interrupted, must wait for completion
    Uninterruptible,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TaskState {
    NotInitialized,
    Ready,
    Running,
    Blocked(BlockedType),
    Zombie,
    Terminated,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TaskType {
    Kernel,
    User,
}

/// ABI Zone structure holding a memory range with an owned ABI module.
pub struct AbiZone {
    pub range: Range<usize>,
    pub abi: Box<dyn AbiModule + Send + Sync>,
}

pub struct Task {
    id: usize,
    pub name: String,
    pub priority: u32,
    pub vcpu: Vcpu,
    /// Kernel context for context switching
    pub kernel_context: KernelContext,
    pub state: TaskState,
    pub task_type: TaskType,
    pub entry: usize,
    pub brk: Option<usize>,    /* Program break (NOT work in Kernel task) */
    pub stack_size: usize,     /* Size of the stack in bytes */
    pub data_size: usize, /* Size of the data segment in bytes (page unit) (NOT work in Kernel task) */
    pub text_size: usize, /* Size of the text segment in bytes (NOT work in Kernel task) */
    pub max_stack_size: usize, /* Maximum size of the stack in bytes */
    pub max_data_size: usize, /* Maximum size of the data segment in bytes */
    pub max_text_size: usize, /* Maximum size of the text segment in bytes */
    pub vm_manager: VirtualMemoryManager,
    /// Managed pages
    ///
    /// Managed pages are freed automatically when the task is terminated.
    pub managed_pages: Vec<ManagedPage>,
    parent_id: Option<usize>, /* Parent task ID */
    children: Vec<usize>,     /* List of child task IDs */
    exit_status: Option<i32>, /* Exit code (for monitoring child task termination) */

    /// Default ABI for this task. Determined from ELF OSABI etc.
    /// Wrapped in Option to allow temporary take() during callbacks
    /// that also need `&mut self` without borrow conflicts.
    pub default_abi: Option<Box<dyn AbiModule + Send + Sync>>,

    /// ABI zones map. Key is the start address of the range.
    pub abi_zones: BTreeMap<usize, AbiZone>,

    /// Virtual File System Manager
    ///
    /// Each task can have its own isolated VfsManager instance for containerization
    /// and namespace isolation. The VfsManager provides:
    ///
    /// - **Filesystem Isolation**: Independent mount point namespaces allowing
    ///   complete filesystem isolation between tasks or containers
    /// - **Selective Sharing**: Arc-based filesystem object sharing enables
    ///   controlled resource sharing while maintaining namespace independence
    /// - **Bind Mount Support**: Advanced bind mount capabilities for flexible
    ///   directory mapping and container orchestration scenarios
    /// - **Security**: Path normalization and validation preventing directory
    ///   traversal attacks and unauthorized filesystem access
    ///
    /// # Usage Patterns
    ///
    /// - `None`: Task uses global filesystem namespace (traditional Unix-like behavior)
    /// - `Some(Arc<VfsManager>)`: Task has isolated filesystem namespace (container-like behavior)
    ///
    /// # Thread Safety
    ///
    /// VfsManager is thread-safe and can be shared between tasks using Arc.
    /// All internal operations use RwLock for concurrent access protection.
    pub vfs: Option<Arc<VfsManager>>,

    // KernelObject table
    pub handle_table: HandleTable,
    /// Time slice (in ticks) for round-robin scheduling. Decremented every tick; when it reaches 0, the scheduler is invoked.
    pub time_slice: u32,
    /// Software timer handlers
    pub software_timers_handlers: Vec<Arc<dyn TimerHandler>>,

    // Wakers for task-specific operations
    /// Waker for sleep operations
    pub sleep_waker: Waker,

    /// Task-local event queue with priority ordering
    pub event_queue: Mutex<crate::ipc::event::TaskEventQueue>,
    /// Event processing enabled flag (similar to interrupt enable/disable)
    pub events_enabled: Mutex<bool>,

    /// Kernel stack window base in shared kernel PT: (slot_index, base_vaddr)
    kernel_stack_window_base: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct ManagedPage {
    pub vaddr: usize,
    pub page: Box<Page>,
}

pub enum CloneFlagsDef {
    Vm = 0b00000001,    // Clone the VM
    Fs = 0b00000010,    // Clone the filesystem
    Files = 0b00000100, // Clone the file descriptors
}

#[derive(Debug, Clone, Copy)]
pub struct CloneFlags {
    raw: u64,
}

impl CloneFlags {
    pub fn new() -> Self {
        CloneFlags { raw: 0 }
    }

    pub fn from_raw(raw: u64) -> Self {
        CloneFlags { raw }
    }

    pub fn set(&mut self, flag: CloneFlagsDef) {
        self.raw |= flag as u64;
    }

    pub fn clear(&mut self, flag: CloneFlagsDef) {
        self.raw &= !(flag as u64);
    }

    pub fn is_set(&self, flag: CloneFlagsDef) -> bool {
        (self.raw & (flag as u64)) != 0
    }

    pub fn get_raw(&self) -> u64 {
        self.raw
    }
}

impl Default for CloneFlags {
    fn default() -> Self {
        let raw = CloneFlagsDef::Fs as u64 | CloneFlagsDef::Files as u64;
        CloneFlags { raw }
    }
}

static TASK_ID: Mutex<usize> = Mutex::new(1);

impl Task {
    pub fn new(name: String, priority: u32, task_type: TaskType) -> Self {
        let mut taskid = TASK_ID.lock();

        let task = Task {
            id: *taskid,
            name,
            priority,
            vcpu: Vcpu::new(match task_type {
                TaskType::Kernel => crate::arch::vcpu::Mode::Kernel,
                TaskType::User => crate::arch::vcpu::Mode::User,
            }),
            kernel_context: KernelContext::new(),
            state: TaskState::NotInitialized,
            task_type,
            entry: 0,
            brk: None,
            stack_size: 0,
            data_size: 0,
            text_size: 0,
            max_stack_size: DEAFAULT_MAX_TASK_STACK_SIZE,
            max_data_size: DEAFAULT_MAX_TASK_DATA_SIZE,
            max_text_size: DEAFAULT_MAX_TASK_TEXT_SIZE,
            vm_manager: VirtualMemoryManager::new(),
            managed_pages: Vec::new(),
            parent_id: None,
            children: Vec::new(),
            exit_status: None,
            default_abi: Some(Box::new(ScarletAbi::default())), // Default ABI
            abi_zones: BTreeMap::new(),
            vfs: None,
            handle_table: HandleTable::new(),
            time_slice: 10, // Assign 10 ticks by default
            software_timers_handlers: Vec::new(),
            // Wakers for task-specific operations
            sleep_waker: Waker::new_interruptible("task_sleep_waker"),
            event_queue: spin::Mutex::new(crate::ipc::event::TaskEventQueue::new()),
            events_enabled: spin::Mutex::new(true), // Events enabled by default
            kernel_stack_window_base: None,
        };

        *taskid += 1;
        task
    }

    pub fn init(&mut self) {
        // Initialize kernel context with the task's entry point
        // The kernel stack is allocated within the KernelContext
        self.kernel_context = KernelContext::new();

        match self.task_type {
            TaskType::Kernel => {
                user_kernel_vm_init(self);
                /* Set sp to the top of the kernel stack */
                self.vcpu.set_sp(KERNEL_VM_STACK_END + 1);
                /* Set pc to the task's entry point */
                self.vcpu.set_pc(self.entry as u64);
            }
            TaskType::User => {
                user_vm_init(self);
                /* Set sp to the top of the user stack */
                self.vcpu.set_sp(USER_STACK_END);
                /* PC will be set when loading the ELF binary */
            }
        }

        // Map kernel stack into shared kernel PT at unique high VA window
        // This must be done after VM initialization so kernel PT is ready
        map_task_kernel_stack_window(self).expect("Failed to map kernel stack window");

        /* Set the task state to Ready */
        self.state = TaskState::Ready;
        self.time_slice = 1;
    }

    pub fn get_id(&self) -> usize {
        self.id
    }

    /// Set the task state
    ///
    /// # Arguments
    /// * `state` - The new task state
    ///
    pub fn set_state(&mut self, state: TaskState) {
        self.state = state;
    }

    /// Get the task state
    ///
    /// # Returns
    /// The task state
    ///
    pub fn get_state(&self) -> TaskState {
        self.state
    }

    /// Get the size of the task.
    ///
    /// # Returns
    /// The size of the task in bytes.
    pub fn get_size(&self) -> usize {
        self.stack_size + self.text_size + self.data_size
    }

    /// Get the program break (NOT work in Kernel task)
    ///
    /// # Returns
    /// The program break address
    pub fn get_brk(&self) -> usize {
        // Return brk if set (represents program end address)
        // Otherwise fallback to legacy size-based calculation for compatibility
        self.brk.unwrap_or(self.text_size + self.data_size)
    }

    /// Set the program break (NOT work in Kernel task)
    ///
    /// # Arguments
    /// * `brk` - The new program break address
    ///
    /// # Returns
    /// If successful, returns Ok(()), otherwise returns an error.
    pub fn set_brk(&mut self, brk: usize) -> Result<(), &'static str> {
        let prev_brk = self.get_brk();
        if brk < prev_brk {
            /* Free pages */
            /* Round address to the page boundary */
            let prev_addr = (prev_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let addr = (brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let num_of_pages = (prev_addr - addr) / PAGE_SIZE;
            self.free_data_pages(addr, num_of_pages);
        } else if brk > prev_brk {
            /* Allocate pages */
            /* Round address to the page boundary */
            let prev_addr = (prev_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let addr = (brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let num_of_pages = (addr - prev_addr) / PAGE_SIZE;

            // crate::println!("[set_brk] Expanding: prev_brk={:#x} -> brk={:#x}", prev_brk, brk);
            // crate::println!("[set_brk] Page allocation: prev_addr={:#x}, addr={:#x}, num_pages={}",
            //     prev_addr, addr, num_of_pages);

            if num_of_pages > 0 {
                match self.vm_manager.search_memory_map(prev_addr) {
                    Some(_existing_map) => {
                        // crate::println!("[set_brk] Existing mapping found: VA {:#x}-{:#x}, skipping allocation",
                        //     existing_map.vmarea.start, existing_map.vmarea.end);
                    }
                    None => {
                        // crate::println!("[set_brk] No existing mapping, allocating {} pages at {:#x}",
                        //     num_of_pages, prev_addr);
                        match self.allocate_data_pages(prev_addr, num_of_pages) {
                            Ok(_) => {
                                // crate::println!("[set_brk] Successfully allocated {} pages", num_of_pages);
                            }
                            Err(_e) => {
                                // crate::println!("[set_brk] Failed to allocate pages: {}", e);
                                return Err("Failed to allocate pages");
                            }
                        }
                    }
                }
            }
        }
        self.brk = Some(brk);
        Ok(())
    }

    /// Allocate pages for the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    /// * `segment` - The segment type to allocate pages
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    /// # Note
    /// This function don't increment the size of the task.
    /// You must increment the size of the task manually.
    ///
    pub fn allocate_pages(
        &mut self,
        vaddr: usize,
        num_of_pages: usize,
        permissions: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        if vaddr % PAGE_SIZE != 0 {
            return Err("Address is not page aligned");
        }

        // Allocate each page independently so they can be freed individually
        let mut first_map: Option<VirtualMemoryMap> = None;
        for i in 0..num_of_pages {
            let page = allocate_page();
            let page_vaddr = vaddr + i * PAGE_SIZE;
            let paddr = page.as_ref() as *const Page as usize;

            // Each page gets its own VMA (1 page = 1 VMA)
            let mmap = VirtualMemoryMap {
                pmarea: MemoryArea {
                    start: paddr,
                    end: paddr + PAGE_SIZE - 1,
                },
                vmarea: MemoryArea {
                    start: page_vaddr,
                    end: page_vaddr + PAGE_SIZE - 1,
                },
                permissions,
                is_shared: false,
                owner: None,
            };
            self.vm_manager
                .add_memory_map(mmap.clone())
                .map_err(|e| panic!("Failed to add memory map: {}", e))?;

            if first_map.is_none() {
                first_map = Some(mmap);
            }

            self.add_managed_page(ManagedPage { vaddr: page_vaddr, page });
        }

        first_map.ok_or("Failed to allocate pages")
    }

    /// Free pages for the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    pub fn free_pages(&mut self, vaddr: usize, num_of_pages: usize) {
        let asid = self.vm_manager.get_asid();

        // Collect pages to free first, then unmap
        let mut pages_to_free = Vec::new();

        for p in 0..num_of_pages {
            let page_vaddr = vaddr + p * PAGE_SIZE;

            // Remove the VMA for this page
            self.vm_manager.remove_memory_map_by_addr(page_vaddr);

            // Remove and free the managed page
            if let Some(free_page) = self.remove_managed_page(page_vaddr) {
                pages_to_free.push((page_vaddr, free_page.page));
            }
        }

        // Now unmap from page table (separate loop to avoid borrow conflict)
        if let Some(root_pagetable) = self.vm_manager.get_root_page_table() {
            for (page_vaddr, page) in pages_to_free {
                root_pagetable.unmap(asid, page_vaddr);
                free_boxed_page(page);
            }
        }
    }

    /// Allocate text pages for the task. And increment the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    pub fn allocate_text_pages(
        &mut self,
        vaddr: usize,
        num_of_pages: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Text.default_permissions();
        let res = self.allocate_pages(vaddr, num_of_pages, permissions);
        if res.is_ok() {
            self.text_size += num_of_pages * PAGE_SIZE;
        }
        res
    }

    /// Free text pages for the task. And decrement the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    ///
    pub fn free_text_pages(&mut self, vaddr: usize, num_of_pages: usize) {
        self.free_pages(vaddr, num_of_pages);
        self.text_size -= num_of_pages * PAGE_SIZE;
    }

    /// Allocate stack pages for the task. And increment the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    pub fn allocate_stack_pages(
        &mut self,
        vaddr: usize,
        num_of_pages: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Stack.default_permissions();
        let res = self.allocate_pages(vaddr, num_of_pages, permissions)?;
        self.stack_size += num_of_pages * PAGE_SIZE;
        Ok(res)
    }

    /// Free stack pages for the task. And decrement the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    ///
    pub fn free_stack_pages(&mut self, vaddr: usize, num_of_pages: usize) {
        self.free_pages(vaddr, num_of_pages);
        self.stack_size -= num_of_pages * PAGE_SIZE;
    }

    /// Allocate data pages for the task. And increment the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    pub fn allocate_data_pages(
        &mut self,
        vaddr: usize,
        num_of_pages: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Data.default_permissions();
        let res = self.allocate_pages(vaddr, num_of_pages, permissions)?;
        self.data_size += num_of_pages * PAGE_SIZE;
        Ok(res)
    }

    /// Free data pages for the task. And decrement the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    ///
    pub fn free_data_pages(&mut self, vaddr: usize, num_of_pages: usize) {
        self.free_pages(vaddr, num_of_pages);
        self.data_size -= num_of_pages * PAGE_SIZE;
    }

    /// Allocate guard pages for the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    /// # Note
    /// Gurad pages are not allocated in the physical memory space.
    /// This function only maps the pages to the virtual memory space.
    ///
    pub fn allocate_guard_pages(
        &mut self,
        vaddr: usize,
        num_of_pages: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Guard.default_permissions();
        let mmap = VirtualMemoryMap {
            pmarea: MemoryArea { start: 0, end: 0 },
            vmarea: MemoryArea {
                start: vaddr,
                end: vaddr + num_of_pages * PAGE_SIZE - 1,
            },
            permissions,
            is_shared: VirtualMemoryRegion::Guard.is_shareable(), // Guard pages can be shared
            owner: None,
        };
        Ok(mmap)
    }

    /// Add pages to the task
    ///
    /// # Arguments
    /// * `pages` - The managed page to add
    ///
    /// # Note
    /// Pages added as ManagedPage of the Task will be automatically freed when the Task is terminated.
    /// So, you must not free them by calling free_raw_pages/free_boxed_pages manually.
    ///
    pub fn add_managed_page(&mut self, pages: ManagedPage) {
        self.managed_pages.push(pages);
    }

    /// Get managed page
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address of the page
    ///
    /// # Returns
    /// The managed page if found, otherwise None
    ///
    #[allow(dead_code)]
    fn get_managed_page(&self, vaddr: usize) -> Option<&ManagedPage> {
        for page in &self.managed_pages {
            if page.vaddr == vaddr {
                return Some(page);
            }
        }
        None
    }

    /// Remove managed page
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address of the page
    ///
    /// # Returns
    /// The removed managed page if found, otherwise None
    ///
    pub fn remove_managed_page(&mut self, vaddr: usize) -> Option<crate::task::ManagedPage> {
        for i in 0..self.managed_pages.len() {
            if self.managed_pages[i].vaddr == vaddr {
                let page = self.managed_pages.remove(i);
                return Some(page);
            }
        }
        None
    }

    // Set the entry point
    pub fn set_entry_point(&mut self, entry: usize) {
        self.vcpu.set_pc(entry as u64);
    }

    /// Get the parent ID
    ///
    /// # Returns
    /// The parent task ID, or None if there is no parent
    pub fn get_parent_id(&self) -> Option<usize> {
        self.parent_id
    }

    /// Set the parent task
    ///
    /// # Arguments
    /// * `parent_id` - The ID of the parent task
    pub fn set_parent_id(&mut self, parent_id: usize) {
        self.parent_id = Some(parent_id);
    }

    /// Add a child task
    ///
    /// # Arguments
    /// * `child_id` - The ID of the child task
    pub fn add_child(&mut self, child_id: usize) {
        if !self.children.contains(&child_id) {
            self.children.push(child_id);
        }
    }

    /// Remove a child task
    ///
    /// # Arguments
    /// * `child_id` - The ID of the child task to remove
    ///
    /// # Returns
    /// true if the removal was successful, false if the child task was not found
    pub fn remove_child(&mut self, child_id: usize) -> bool {
        if let Some(pos) = self.children.iter().position(|&id| id == child_id) {
            self.children.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get the list of child tasks
    ///
    /// # Returns
    /// A vector of child task IDs
    pub fn get_children(&self) -> &Vec<usize> {
        &self.children
    }

    /// Set the exit status
    ///
    /// # Arguments
    /// * `status` - The exit status
    pub fn set_exit_status(&mut self, status: i32) {
        self.exit_status = Some(status);
    }

    /// Get the exit status
    ///
    /// # Returns
    /// The exit status, or None if not set
    pub fn get_exit_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// Resolve the ABI to use for the given address
    ///
    /// This method returns a mutable reference to the ABI module that should be used
    /// for a system call issued from the given address. It searches the ABI zones map
    /// and returns the appropriate ABI, falling back to the default ABI if no zone matches.
    ///
    /// # Arguments
    /// * `addr` - The program counter address where the system call was issued
    ///
    /// # Returns
    /// A mutable reference to the ABI module to use
    pub fn resolve_abi_mut(&mut self, addr: usize) -> &mut (dyn AbiModule + Send + Sync) {
        // Search for the zone containing addr using efficient BTreeMap range query
        if let Some((_start, zone)) = self.abi_zones.range_mut(..=addr).next_back() {
            if zone.range.contains(&addr) {
                return zone.abi.as_mut();
            }
        }
        // No zone found, return default ABI
        self.default_abi
            .as_deref_mut()
            .expect("default_abi not set")
    }

    /// Get an immutable reference to the default ABI
    pub fn default_abi_ref(&self) -> &(dyn AbiModule + Send + Sync) {
        self.default_abi.as_deref().expect("default_abi not set")
    }

    /// Get a mutable reference to the default ABI
    pub fn default_abi_mut(&mut self) -> &mut (dyn AbiModule + Send + Sync) {
        self.default_abi
            .as_deref_mut()
            .expect("default_abi not set")
    }

    /// Temporarily take ownership of the default ABI to run a closure that also needs &mut self
    pub fn with_default_abi_mut<R>(
        &mut self,
        f: impl FnOnce(&mut (dyn AbiModule + Send + Sync), &mut Task) -> R,
    ) -> R {
        let mut abi = self.default_abi.take().expect("default_abi not set");
        let r = f(abi.as_mut(), self);
        self.default_abi = Some(abi);
        r
    }

    /// Get the file descriptor table
    ///
    /// # Returns
    /// A reference to the file descriptor table
    ///
    /// Clone this task, creating a near-identical copy
    ///
    /// # Arguments
    ///
    /// # Returns
    /// The cloned task
    ///
    /// # Errors
    /// If the task cannot be cloned, an error is returned.
    ///
    pub fn clone_task(&mut self, flags: CloneFlags) -> Result<Task, &'static str> {
        // Create a new task (but don't call init() yet)
        let mut child = Task::new(self.name.clone(), self.priority, self.task_type);

        // First, set up the virtual memory manager with the same ASID allocation
        match self.task_type {
            TaskType::Kernel => {
                // For kernel tasks, we need to call init to set up the kernel VM
                child.init();
            }
            TaskType::User => {
                if !flags.is_set(CloneFlagsDef::Vm) {
                    // For user tasks, manually set up VM without calling init()
                    // to avoid creating new stack that would overwrite parent's stack content
                    let asid = alloc_virtual_address_space();
                    child.vm_manager.set_asid(asid);
                } else {
                    // CLONE_VM: share the same address space via Arc<VirtualMemoryManager>
                    child.vm_manager = self.vm_manager.clone();
                }
            }
        }

        if !flags.is_set(CloneFlagsDef::Vm) {
            // Copy or share memory maps from parent to child without cloning lists
            self.vm_manager.memmaps_iter_with(|iter| {
                for mmap in iter {
                    let num_pages =
                        (mmap.vmarea.end - mmap.vmarea.start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
                    if num_pages == 0 {
                        continue;
                    }

                    let vaddr = mmap.vmarea.start;
                    if mmap.is_shared {
                        // Shared memory regions: just reference the same physical pages
                        let shared_mmap = VirtualMemoryMap {
                            pmarea: mmap.pmarea,
                            vmarea: mmap.vmarea,
                            permissions: mmap.permissions,
                            is_shared: true,
                            owner: mmap.owner.clone(),
                        };
                        child
                            .vm_manager
                            .add_memory_map(shared_mmap.clone())
                            .map_err(|_| "Failed to add shared memory map to child task")?;

                        // Pre-map trampoline page if applicable
                        if mmap.vmarea.start == 0xffff_ffff_ffff_f000 {
                            if let Some(root_pagetable) = child.vm_manager.get_root_page_table() {
                                root_pagetable
                                    .map_memory_area(child.vm_manager.get_asid(), shared_mmap)
                                    .map_err(|_| "Failed to map trampoline page")?;
                            }
                        }
                    } else {
                        // Private memory regions: allocate new pages and copy contents
                        let permissions = mmap.permissions;

                        // Allocate each page independently (1 page = 1 VMA)
                        for i in 0..num_pages {
                            let page = allocate_page();
                            let page_vaddr = vaddr + i * PAGE_SIZE;
                            let paddr = page.as_ref() as *const Page as usize;

                            // Copy original contents
                            let src_page_addr = mmap.pmarea.start + i * PAGE_SIZE;
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    src_page_addr as *const u8,
                                    paddr as *mut u8,
                                    PAGE_SIZE,
                                );
                            }

                            // Each page gets its own VMA
                            let new_mmap = VirtualMemoryMap {
                                pmarea: MemoryArea {
                                    start: paddr,
                                    end: paddr + PAGE_SIZE - 1,
                                },
                                vmarea: MemoryArea {
                                    start: page_vaddr,
                                    end: page_vaddr + PAGE_SIZE - 1,
                                },
                                permissions,
                                is_shared: false,
                                owner: mmap.owner.clone(),
                            };

                            child
                                .vm_manager
                                .add_memory_map(new_mmap)
                                .map_err(|_| "Failed to add memory map to child task")?;

                            child.add_managed_page(ManagedPage {
                                vaddr: page_vaddr,
                                page,
                            });
                        }
                    }
                }
                Ok::<(), &'static str>(())
            })?;
        }

        // Copy register states
        self.vcpu.copy_iregs_to(&mut child.vcpu.iregs);

        // Clone the default ABI and ABI zones
        child.default_abi = Some(
            self.default_abi
                .as_ref()
                .expect("default_abi not set")
                .clone_boxed(),
        );
        // Clone ABI zones (each zone contains a boxed ABI that needs to be cloned)
        for (start, zone) in &self.abi_zones {
            let new_zone = AbiZone {
                range: zone.range.clone(),
                abi: zone.abi.clone_boxed(),
            };
            child.abi_zones.insert(*start, new_zone);
        }
        // Notify child's default ABI instance that cloning has completed
        // Take and restore to avoid mutable aliasing with &mut child
        if let Some(mut abi_boxed) = child.default_abi.take() {
            let _ = abi_boxed.on_task_cloned(self, &mut child, flags);
            child.default_abi = Some(abi_boxed);
        }

        // Copy state such as data size
        child.stack_size = self.stack_size;
        child.data_size = self.data_size;
        child.text_size = self.text_size;
        child.max_stack_size = self.max_stack_size;
        child.max_data_size = self.max_data_size;
        child.max_text_size = self.max_text_size;

        // Set the same entry point and PC
        child.entry = self.entry;
        child.vcpu.set_pc(self.vcpu.get_pc());

        if flags.is_set(CloneFlagsDef::Files) {
            // Clone the file descriptor table
            child.handle_table = self.handle_table.clone();
        }

        if flags.is_set(CloneFlagsDef::Fs) {
            // Clone the filesystem manager
            if let Some(vfs) = &self.vfs {
                child.vfs = Some(vfs.clone());
                // Current working directory is managed within VfsManager
            } else {
                child.vfs = None;
            }
        }

        // Initialize kernel context
        child.kernel_context = KernelContext::new();
        // Set the state to Ready
        child.state = self.state;

        // Set parent-child relationship
        child.set_parent_id(self.id);
        self.add_child(child.get_id());

        Ok(child)
    }

    /// Exit the task
    ///
    /// # Arguments
    /// * `status` - The exit status
    ///
    pub fn exit(&mut self, status: i32) {
        // Close all open handles when task exits
        self.handle_table.close_all();
        // Let current ABI perform exit-time cleanup (Linux: clear_child_tid, robust list, etc.)
        // Use take/restore to avoid aliasing &mut self and &mut field
        self.with_default_abi_mut(|abi, task| abi.on_task_exit(task));

        match self.parent_id {
            Some(parent_id) => {
                if get_scheduler().get_task_by_id(parent_id).is_none() {
                    // crate::println!("Task {}: Parent {} not found, terminating", self.id, parent_id);
                    self.state = TaskState::Terminated;
                    return;
                }
                /* Set the exit status */
                self.set_exit_status(status);
                self.state = TaskState::Zombie;

                // TODO: Notify parent via ABI-specific mechanism
                // crate::println!("Task {}: Set to Zombie state, parent {}", self.id, parent_id);
            }
            None => {
                /* If the task has no parent, it is terminated */
                // crate::println!("Task {}: No parent, terminating", self.id);
                self.state = TaskState::Terminated;
            }
        }

        // Task cleanup completed - ABI module handles event cleanup

        if mytask().is_none() || mytask().unwrap().get_id() != self.id {
            // Not the current task, nothing more to do
            return;
        }

        // The scheduler will handle saving the current task state internally
        if let Some(current_task) = mytask() {
            get_scheduler().schedule(current_task.get_trapframe());
        }
    }

    /// Wait for a child task to exit and collect its status
    ///
    /// # Arguments
    /// * `child_id` - The ID of the child task to wait for
    ///
    /// # Returns
    /// The exit status of the child task, or an error if the child is not found or not in Zombie state
    pub fn wait(&mut self, child_id: usize) -> Result<i32, WaitError> {
        if !self.children.contains(&child_id) {
            crate::println!("[Task {}] wait: No such child task: {}", self.id, child_id);
            return Err(WaitError::NoSuchChild("No such child task".to_string()));
        }

        let scheduler = get_scheduler();
        if let Some(child_task) = scheduler.get_task_by_id(child_id) {
            let status = child_task.get_exit_status().unwrap_or(-1);
            if child_task.get_state() != TaskState::Zombie {
                return Err(WaitError::ChildNotExited(
                    "Child has not exited or is not a zombie".to_string(),
                ));
            }
            // Ensure the child will be removed when the scheduler sees it again.
            child_task.set_state(TaskState::Terminated);
            // Drop child resources now (vm_manager, managed_pages, etc.).
            self.remove_child(child_id);
            scheduler.cleanup_zombie_task(child_id);
            Ok(status)
        } else {
            Err(WaitError::ChildTaskNotFound(
                "Child task not found".to_string(),
            ))
        }
    }

    /// Sleep the current task for the specified number of ticks.
    /// This blocks the task and registers a timer to wake it up.
    ///
    /// # Arguments
    /// * `trapframe` - The trapframe of the current CPU state
    /// * `ticks` - The number of ticks to sleep
    ///
    pub fn sleep(&mut self, trapframe: &mut Trapframe, ticks: u64) {
        struct SleepWakerHandler {
            task_id: usize,
            _start_tick: u64,
        }

        impl TimerHandler for SleepWakerHandler {
            fn on_timer_expired(self: Arc<Self>, _context: usize) {
                if let Some(task) = get_scheduler().get_task_by_id(self.task_id) {
                    let handler: Arc<dyn TimerHandler> = self.clone();
                    task.remove_software_timer_handler(&handler);
                    // crate::println!("Task {} woke up after {} ticks", self.task_id, get_tick() - self.start_tick);
                    let waker = get_waitpid_waker(self.task_id);
                    waker.wake_all();
                }
            }
        }

        let wake_tick = get_tick() + ticks;
        let handler: Arc<dyn crate::timer::TimerHandler> = Arc::new(SleepWakerHandler {
            task_id: self.id,
            _start_tick: get_tick(),
        });
        add_timer(wake_tick, &handler, 0);

        self.add_software_timer_handler(handler);
        let waker = get_waitpid_waker(self.id);
        waker.wait(self.get_id(), trapframe);
    }

    // VFS Helper Methods

    /// Set the VFS manager
    ///
    /// # Arguments
    /// * `vfs` - The VfsManager to set as the VFS
    pub fn set_vfs(&mut self, vfs: Arc<VfsManager>) {
        self.vfs = Some(vfs);
    }

    /// Get a reference to the VFS
    pub fn get_vfs(&self) -> Option<&Arc<VfsManager>> {
        self.vfs.as_ref()
    }

    pub fn add_software_timer_handler(&mut self, timer: Arc<dyn TimerHandler>) {
        self.software_timers_handlers.push(timer);
    }

    pub fn remove_software_timer_handler(&mut self, timer: &Arc<dyn TimerHandler>) {
        if let Some(pos) = self
            .software_timers_handlers
            .iter()
            .position(|x| Arc::ptr_eq(x, timer))
        {
            self.software_timers_handlers.remove(pos);
        }
    }

    /// Enable event processing for this task (similar to enabling interrupts)
    pub fn enable_events(&self) {
        let mut enabled = self.events_enabled.lock();
        *enabled = true;
    }

    /// Disable event processing for this task (similar to disabling interrupts)
    pub fn disable_events(&self) {
        let mut enabled = self.events_enabled.lock();
        *enabled = false;
    }

    /// Check if events are enabled for this task
    pub fn events_enabled(&self) -> bool {
        *self.events_enabled.lock()
    }

    /// Process pending events if events are enabled
    /// This should be called by the scheduler before resuming the task
    ///
    /// Following signal-like semantics:
    /// - Process a limited number of events per scheduler cycle to avoid starvation
    /// - Critical events (like KILL) are processed immediately
    /// - Normal events are batched and processed in priority order
    pub fn process_pending_events(&self) -> Result<(), &'static str> {
        // Check if events are enabled
        if !self.events_enabled() {
            return Ok(()); // Events disabled, skip processing
        }

        // Delegate to ABI module for event processing
        let abi = self.default_abi_ref();
        const MAX_EVENTS_PER_CYCLE: usize = 8; // Prevent scheduler starvation
        let mut processed_count = 0;

        // Process events with limits to prevent infinite loops
        while processed_count < MAX_EVENTS_PER_CYCLE {
            let event = {
                let mut queue = self.event_queue.lock();
                queue.dequeue()
            };

            match event {
                Some(event) => {
                    processed_count += 1;

                    // Check if this is a critical event that requires immediate attention
                    let is_critical = self.is_critical_event(&event);

                    // Let ABI handle the event
                    abi.handle_event(event, self.id as u32)?;

                    // Check if events were disabled during handling
                    if !self.events_enabled() {
                        break;
                    }

                    // If we processed a critical event, we can stop here
                    // to allow the ABI module to take appropriate action
                    if is_critical {
                        break;
                    }
                }
                None => break, // No more events
            }
        }

        // If we hit the limit and there are still events, the scheduler
        // will call us again on the next cycle
        if processed_count == MAX_EVENTS_PER_CYCLE {
            let queue = self.event_queue.lock();
            if !queue.is_empty() {
                // Log that we're deferring events to next cycle
                // crate::early_println!("Task {}: Deferring {} events to next scheduler cycle",
                //                      self.id, queue.len());
            }
        }

        Ok(())
    }

    /// Check if an event is critical and should be processed immediately
    /// Critical events typically cannot be ignored and affect task state directly
    fn is_critical_event(&self, event: &crate::ipc::event::Event) -> bool {
        use crate::ipc::event::EventPriority;

        // High/Critical priority events are always considered critical
        match event.metadata.priority {
            EventPriority::Critical => return true,
            EventPriority::High => {
                // Some high priority events are critical depending on content
                match &event.content {
                    EventContent::ProcessControl(ProcessControlType::Kill) => true,
                    EventContent::Custom { event_id, .. } => {
                        // Could map specific event IDs to critical signals
                        *event_id == 9 // SIGKILL-like event
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Get a mutable reference to the kernel context for context switching
    pub fn get_kernel_context_mut(&mut self) -> &mut KernelContext {
        &mut self.kernel_context
    }

    /// Get a reference to the kernel context
    pub fn get_kernel_context(&self) -> &KernelContext {
        &self.kernel_context
    }

    /// Get the kernel stack bottom address for this task
    ///
    /// # Returns
    /// The kernel stack bottom address as u64, or 0 if no kernel stack is allocated
    pub fn get_kernel_stack_bottom(&self) -> u64 {
        self.kernel_context.get_kernel_stack_bottom()
    }

    /// Get the kernel stack memory area for this task
    ///
    /// # Returns
    /// The kernel stack memory area as a MemoryArea
    ///
    pub fn get_kernel_stack_memory_area(&self) -> MemoryArea {
        self.kernel_context.get_kernel_stack_memory_area()
    }

    /// Get a mutable reference to the trapframe for this task
    ///
    /// The trapframe contains the user-space register state and is located
    /// at the top of the kernel stack. This provides access to modify the
    /// user context during system calls, interrupts, and context switches.
    ///
    /// # Returns
    /// A mutable reference to the Trapframe
    pub fn get_trapframe(&mut self) -> &mut Trapframe {
        self.kernel_context.get_trapframe()
    }

    /// Internal: set kernel stack window base (slot index and base vaddr)
    pub fn set_kernel_stack_window_base(&mut self, base: Option<(usize, usize)>) {
        self.kernel_stack_window_base = base;
    }

    /// Get kernel stack window base (slot index and base vaddr)
    pub fn get_kernel_stack_window_base(&self) -> Option<(usize, usize)> {
        self.kernel_stack_window_base
    }
}

#[derive(Debug)]
pub enum WaitError {
    NoSuchChild(String),
    ChildNotExited(String),
    ChildTaskNotFound(String),
}

impl WaitError {
    pub fn message(&self) -> &str {
        match self {
            WaitError::NoSuchChild(msg) => msg,
            WaitError::ChildNotExited(msg) => msg,
            WaitError::ChildTaskNotFound(msg) => msg,
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        // Best-effort teardown of kernel stack window mapping
        crate::vm::unmap_task_kernel_stack_window(self);
    }
}

/// Create a new kernel task.
///
/// # Arguments
/// * `name` - The name of the task
/// * `priority` - The priority of the task
/// * `func` - The function to run in the task
///
/// # Returns
/// The new task.
pub fn new_kernel_task(name: String, priority: u32, func: fn()) -> Task {
    let mut task = Task::new(name, priority, TaskType::Kernel);
    task.entry = func as usize;
    task
}

/// Create a new user task.
///
/// # Arguments
/// * `name` - The name of the task
/// * `priority` - The priority of the task
///
/// # Returns
/// The new task.
pub fn new_user_task(name: String, priority: u32) -> Task {
    Task::new(name, priority, TaskType::User)
}

#[cfg(test)]
static mut MOCK_CURRENT_TASK: Option<*mut Task> = None;

#[cfg(test)]
/// Set a mock current task for testing purposes
///
/// This function allows tests to override the return value of mytask()
/// for controlled testing scenarios.
///
/// # Arguments
/// * `task` - The task to return from mytask()
///
/// # Safety
/// The caller must ensure the task pointer remains valid for the duration
/// of the test and that clear_mock_current_task() is called when done.
/// This function is only safe to call in single-threaded test environments.
pub unsafe fn set_mock_current_task(task: &'static mut Task) {
    unsafe {
        MOCK_CURRENT_TASK = Some(task as *mut Task);
    }
}

#[cfg(test)]
/// Clear the mock current task, reverting to normal scheduler behavior
///
/// # Safety
/// This function is only safe to call in single-threaded test environments.
pub unsafe fn clear_mock_current_task() {
    unsafe {
        MOCK_CURRENT_TASK = None;
    }
}

/// Get the current task.
///
/// # Returns
/// The current task if it exists.
pub fn mytask() -> Option<&'static mut Task> {
    #[cfg(test)]
    {
        unsafe {
            if let Some(task_ptr) = MOCK_CURRENT_TASK {
                return Some(&mut *task_ptr);
            }
        }
    }

    let cpu = get_cpu();
    get_scheduler().get_current_task(cpu.get_cpuid())
}

/// Set the current working directory for the current task via VfsManager
///
/// This function sets the current working directory of the calling task
/// using the VfsManager's path-based API.
///
/// # Arguments
/// * `path` - The new working directory path
///
/// # Returns
/// * `true` if successful, `false` if no current task or VfsManager
pub fn set_current_task_cwd(path: String) -> bool {
    if let Some(task) = mytask() {
        if let Some(vfs) = &task.vfs {
            // Use VfsManager to set current working directory
            vfs.set_cwd_by_path(&path).is_ok()
        } else {
            false // No VfsManager available
        }
    } else {
        false
    }
}

/// Internal function to perform kernel context switch between tasks
/// This function is called when a task is first scheduled.
pub fn task_initial_kernel_entrypoint() -> ! {
    let cpu = get_cpu();
    let current_task = get_scheduler().get_current_task(cpu.get_cpuid()).unwrap();
    Scheduler::setup_task_execution(cpu, current_task);
    arch_switch_to_user_space(current_task.get_trapframe());
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use crate::task::CloneFlags;

    #[test_case]
    fn test_set_brk() {
        let mut task = super::new_user_task("Task0".to_string(), 0);
        task.init();
        assert_eq!(task.get_brk(), 0);
        task.set_brk(0x1000).unwrap();
        assert_eq!(task.get_brk(), 0x1000);
        task.set_brk(0x2000).unwrap();
        assert_eq!(task.get_brk(), 0x2000);
        task.set_brk(0x1008).unwrap();
        assert_eq!(task.get_brk(), 0x1008);
        task.set_brk(0x1000).unwrap();
        assert_eq!(task.get_brk(), 0x1000);
    }

    #[test_case]
    fn test_task_parent_child_relationship() {
        let mut parent_task = super::new_user_task("ParentTask".to_string(), 0);
        parent_task.init();

        let mut child_task = super::new_user_task("ChildTask".to_string(), 0);
        child_task.init();

        // Set parent-child relationship
        child_task.set_parent_id(parent_task.get_id());
        parent_task.add_child(child_task.get_id());

        // Verify parent-child relationship
        assert_eq!(child_task.get_parent_id(), Some(parent_task.get_id()));
        assert!(parent_task.get_children().contains(&child_task.get_id()));

        // Remove child and verify
        assert!(parent_task.remove_child(child_task.get_id()));
        assert!(!parent_task.get_children().contains(&child_task.get_id()));
    }

    #[test_case]
    fn test_task_exit_status() {
        let mut task = super::new_user_task("TaskWithExitStatus".to_string(), 0);
        task.init();

        // Verify initial exit status is None
        assert_eq!(task.get_exit_status(), None);

        // Set and verify exit status
        task.set_exit_status(0);
        assert_eq!(task.get_exit_status(), Some(0));

        task.set_exit_status(1);
        assert_eq!(task.get_exit_status(), Some(1));
    }

    #[test_case]
    fn test_clone_task_memory_copy() {
        let mut parent_task = super::new_user_task("ParentTask".to_string(), 0);
        parent_task.init();

        // Allocate some memory pages for the parent task
        let vaddr = 0x1000;
        let num_pages = 2;
        let _mmap = parent_task.allocate_data_pages(vaddr, num_pages).unwrap();

        // Get the first page's physical address for testing
        let first_page_paddr = parent_task
            .vm_manager
            .translate_vaddr(vaddr)
            .expect("Failed to translate vaddr");

        // Write test data to parent's memory
        let test_data: [u8; 8] = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        unsafe {
            let dst_ptr = first_page_paddr as *mut u8;
            core::ptr::copy_nonoverlapping(test_data.as_ptr(), dst_ptr, test_data.len());
        }

        // Get parent memory map count before cloning
        let parent_memmap_count = parent_task.vm_manager.memmap_len();
        let parent_id = parent_task.get_id();

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // Get child memory map count after cloning
        let child_memmap_count = child_task.vm_manager.memmap_len();

        // Verify that the number of memory maps are identical
        assert_eq!(
            child_memmap_count, parent_memmap_count,
            "Child should have the same number of memory maps as parent: child={}, parent={}",
            child_memmap_count, parent_memmap_count
        );

        // Verify parent-child relationship was established
        assert_eq!(child_task.get_parent_id(), Some(parent_id));
        assert!(parent_task.get_children().contains(&child_task.get_id()));

        // Verify memory sizes were copied
        assert_eq!(child_task.stack_size, parent_task.stack_size);
        assert_eq!(child_task.data_size, parent_task.data_size);
        assert_eq!(child_task.text_size, parent_task.text_size);

        // Find the corresponding memory map in child that matches the first page
        // (with new design, each page has its own VMA)
        let child_first_page_mmap = {
            let mut found = None;
            child_task.vm_manager.with_memmaps(|mm| {
                for m in mm.values() {
                    if m.vmarea.start == vaddr && m.vmarea.end == vaddr + crate::environment::PAGE_SIZE - 1 {
                        found = Some(m.clone());
                        break;
                    }
                }
            });
            found.expect("First page memory map not found in child task")
        };

        // Verify the virtual memory ranges match for the first page
        assert_eq!(child_first_page_mmap.vmarea.start, vaddr);
        assert_eq!(child_first_page_mmap.vmarea.end, vaddr + crate::environment::PAGE_SIZE - 1);

        // Verify the data was copied correctly
        unsafe {
            let parent_ptr = first_page_paddr as *const u8;
            let child_ptr = child_first_page_mmap.pmarea.start as *const u8;

            // Check that physical addresses are different (separate memory)
            assert_ne!(
                parent_ptr, child_ptr,
                "Parent and child should have different physical memory"
            );

            // Check that the data content is identical
            for i in 0..test_data.len() {
                let parent_byte = *parent_ptr.offset(i as isize);
                let child_byte = *child_ptr.offset(i as isize);
                assert_eq!(parent_byte, child_byte, "Data mismatch at offset {}", i);
            }
        }

        // Verify that modifying parent's memory doesn't affect child's memory
        unsafe {
            let parent_ptr = first_page_paddr as *mut u8;
            let original_value = *parent_ptr;
            *parent_ptr = 0xFF; // Modify first byte in parent

            let child_ptr = child_first_page_mmap.pmarea.start as *const u8;
            let child_first_byte = *child_ptr;

            // Child's first byte should still be the original value
            assert_eq!(
                child_first_byte, original_value,
                "Child memory should be independent from parent"
            );
        }

        // Verify register states were copied
        assert_eq!(child_task.vcpu.get_pc(), parent_task.vcpu.get_pc());

        // Verify entry point was copied
        assert_eq!(child_task.entry, parent_task.entry);

        // Verify state was copied
        assert_eq!(child_task.state, parent_task.state);

        // Verify that both tasks have the correct number of managed pages
        assert!(
            child_task.managed_pages.len() >= num_pages,
            "Child should have at least the test pages in managed pages"
        );
    }

    #[test_case]
    fn test_clone_task_stack_copy() {
        let mut parent_task = super::new_user_task("ParentWithStack".to_string(), 0);
        parent_task.init();

        // Find the last stack page in parent (the one ending at USER_STACK_END - 1)
        // With the new design, each stack page has its own VMA
        let stack_mmap = {
            let mut found = None;
            parent_task.vm_manager.with_memmaps(|mm| {
                for mmap in mm.values() {
                    use crate::vm::vmem::VirtualMemoryRegion;
                    if mmap.vmarea.end == crate::environment::USER_STACK_END - 1
                        && mmap.permissions == VirtualMemoryRegion::Stack.default_permissions()
                    {
                        found = Some(mmap.clone());
                        break;
                    }
                }
            });
            found.expect("Stack top page not found in parent task")
        };

        // Write test data to parent's stack (in the top page)
        let stack_test_data: [u8; 16] = [
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
            0x99, 0x00,
        ];
        unsafe {
            // Write at the beginning of the top stack page
            let stack_ptr = stack_mmap.pmarea.start as *mut u8;
            core::ptr::copy_nonoverlapping(
                stack_test_data.as_ptr(),
                stack_ptr,
                stack_test_data.len(),
            );
        }

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // Find the corresponding stack page in child
        let child_stack_mmap = {
            let mut found = None;
            child_task.vm_manager.with_memmaps(|mm| {
                for mmap in mm.values() {
                    use crate::vm::vmem::VirtualMemoryRegion;
                    if mmap.vmarea.start == stack_mmap.vmarea.start
                        && mmap.vmarea.end == stack_mmap.vmarea.end
                        && mmap.permissions == VirtualMemoryRegion::Stack.default_permissions()
                    {
                        found = Some(mmap.clone());
                        break;
                    }
                }
            });
            found.expect("Stack top page not found in child task")
        };

        // Verify that stack content was copied correctly
        unsafe {
            let parent_stack_ptr = stack_mmap.pmarea.start as *const u8;
            let child_stack_ptr = child_stack_mmap.pmarea.start as *const u8;

            // Check that physical addresses are different (separate memory)
            assert_ne!(
                parent_stack_ptr, child_stack_ptr,
                "Parent and child should have different stack physical memory"
            );

            // Check that the stack data content is identical
            for i in 0..stack_test_data.len() {
                let parent_byte = *parent_stack_ptr.offset(i as isize);
                let child_byte = *child_stack_ptr.offset(i as isize);
                assert_eq!(
                    parent_byte, child_byte,
                    "Stack data mismatch at offset {}: parent={:#x}, child={:#x}",
                    i, parent_byte, child_byte
                );
            }
        }

        // Verify that modifying parent's stack doesn't affect child's stack
        unsafe {
            let parent_stack_ptr = stack_mmap.pmarea.start as *mut u8;
            let original_value = *parent_stack_ptr;
            *parent_stack_ptr = 0xFE; // Modify first byte in parent stack

            let child_stack_ptr = child_stack_mmap.pmarea.start as *const u8;
            let child_first_byte = *child_stack_ptr;

            // Child's first byte should still be the original value
            assert_eq!(
                child_first_byte, original_value,
                "Child stack should be independent from parent stack"
            );
        }

        // Verify stack sizes match
        assert_eq!(
            child_task.stack_size, parent_task.stack_size,
            "Child and parent should have the same stack size"
        );
    }

    #[test_case]
    fn test_clone_task_shared_memory() {
        use crate::environment::PAGE_SIZE;
        use crate::mem::page::allocate_raw_pages;
        use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};

        let mut parent_task = super::new_user_task("ParentWithShared".to_string(), 0);
        parent_task.init();

        // Manually add a shared memory region to test sharing behavior
        let shared_vaddr = 0x5000;
        let num_pages = 1;
        let pages = allocate_raw_pages(num_pages);
        let paddr = pages as usize;

        let shared_mmap = VirtualMemoryMap {
            pmarea: MemoryArea {
                start: paddr,
                end: paddr + PAGE_SIZE - 1,
            },
            vmarea: MemoryArea {
                start: shared_vaddr,
                end: shared_vaddr + PAGE_SIZE - 1,
            },
            permissions: VirtualMemoryPermission::Read as usize
                | VirtualMemoryPermission::Write as usize,
            is_shared: true, // This should be shared between parent and child
            owner: None,
        };

        // Add shared memory map to parent
        parent_task
            .vm_manager
            .add_memory_map(shared_mmap.clone())
            .unwrap();

        // Write test data to shared memory
        let test_data: [u8; 8] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22];
        unsafe {
            let shared_ptr = paddr as *mut u8;
            core::ptr::copy_nonoverlapping(test_data.as_ptr(), shared_ptr, test_data.len());
        }

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // Find the shared memory map in child
        let child_shared_mmap = {
            let mut found = None;
            child_task.vm_manager.with_memmaps(|mm| {
                for mmap in mm.values() {
                    if mmap.vmarea.start == shared_vaddr && mmap.is_shared {
                        found = Some(mmap.clone());
                        break;
                    }
                }
            });
            found.expect("Shared memory map not found in child task")
        };

        // Verify that the physical addresses are the same (shared memory)
        assert_eq!(
            child_shared_mmap.pmarea.start, shared_mmap.pmarea.start,
            "Shared memory should have the same physical address in parent and child"
        );

        // Verify that the virtual addresses are the same
        assert_eq!(child_shared_mmap.vmarea.start, shared_mmap.vmarea.start);
        assert_eq!(child_shared_mmap.vmarea.end, shared_mmap.vmarea.end);

        // Verify that is_shared flag is preserved
        assert!(
            child_shared_mmap.is_shared,
            "Shared memory should remain marked as shared"
        );

        // Verify that modifying shared memory from child affects parent
        unsafe {
            let child_shared_ptr = child_shared_mmap.pmarea.start as *mut u8;
            let original_value = *child_shared_ptr;
            *child_shared_ptr = 0xFF; // Modify first byte through child reference

            let parent_shared_ptr = shared_mmap.pmarea.start as *const u8;
            let parent_first_byte = *parent_shared_ptr;

            // Parent should see the change made by child (shared memory)
            assert_eq!(
                parent_first_byte, 0xFF,
                "Parent should see changes made through child's shared memory reference"
            );

            // Restore original value
            *child_shared_ptr = original_value;
        }

        // Verify that the shared data content is accessible from both
        unsafe {
            let child_ptr = child_shared_mmap.pmarea.start as *const u8;
            let parent_ptr = shared_mmap.pmarea.start as *const u8;

            // Check that the data content is identical and accessible from both
            for i in 0..test_data.len() {
                let parent_byte = *parent_ptr.offset(i as isize);
                let child_byte = *child_ptr.offset(i as isize);
                assert_eq!(
                    parent_byte, child_byte,
                    "Shared memory data should be identical from both parent and child views"
                );
            }
        }
    }

    #[test_case]
    fn test_clone_task_with_clone_vm_shares_address_space() {
        use crate::environment::PAGE_SIZE;

        let mut parent = super::new_user_task("ParentCloneVm".to_string(), 0);
        parent.init();

        // Allocate one page initially in the parent
        let base_vaddr = 0x4000;
        parent.allocate_data_pages(base_vaddr, 1).unwrap();
        let parent_len_before = parent.vm_manager.memmap_len();

        // Clone with CLONE_VM flag (share the address space only)
        let mut flags = super::CloneFlags::new();
        flags.set(super::CloneFlagsDef::Vm);
        let child = parent.clone_task(flags).unwrap();

        // Indirectly verify that both share the same ASID/address space
        assert_eq!(child.vm_manager.get_asid(), parent.vm_manager.get_asid());
        assert_eq!(child.vm_manager.memmap_len(), parent_len_before);

        // Adding another page in the parent should be immediately visible to the child
        parent
            .allocate_data_pages(base_vaddr + PAGE_SIZE, 1)
            .unwrap();
        assert_eq!(
            child.vm_manager.memmap_len(),
            parent.vm_manager.memmap_len()
        );

        // Managed pages are per-task; child should not acquire new managed pages
        // when sharing VM (physical memory isn't privately managed by the child)
        assert!(child.managed_pages.len() <= parent.managed_pages.len());
    }
}
