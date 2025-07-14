//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

pub mod syscall;
pub mod elf_loader;

extern crate alloc;

use alloc::{boxed::Box, string::{String, ToString}, sync::Arc, vec::Vec};
use spin::Mutex;

use crate::{arch::{get_cpu, vcpu::Vcpu, vm::alloc_virtual_address_space}, environment::{DEAFAULT_MAX_TASK_DATA_SIZE, DEAFAULT_MAX_TASK_STACK_SIZE, DEAFAULT_MAX_TASK_TEXT_SIZE, KERNEL_VM_STACK_END, PAGE_SIZE}, fs::VfsManager, mem::page::{allocate_raw_pages, free_boxed_page, Page}, object::handle::HandleTable, sched::scheduler::get_scheduler, vm::{manager::VirtualMemoryManager, user_kernel_vm_init, user_vm_init, vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryRegion}}};
use crate::abi::{scarlet::ScarletAbi, AbiModule};
use crate::sync::waker::Waker;
use alloc::collections::BTreeMap;
use spin::Once;

/// Global registry of task-specific wakers for waitpid
static TASK_WAKERS: Once<Mutex<BTreeMap<usize, Waker>>> = Once::new();

/// Global registry of parent task wakers for waitpid(-1) operations
/// Each parent task has a waker that gets triggered when any of its children exit
static PARENT_WAKERS: Once<Mutex<BTreeMap<usize, Waker>>> = Once::new();

/// Initialize the task wakers registry
fn init_task_wakers() -> Mutex<BTreeMap<usize, Waker>> {
    Mutex::new(BTreeMap::new())
}

/// Initialize the parent waker registry
fn init_parent_wakers() -> Mutex<BTreeMap<usize, Waker>> {
    Mutex::new(BTreeMap::new())
}

/// Get or create a waker for a specific task
/// 
/// This function returns a reference to the waker associated with the given task ID.
/// If no waker exists for the task, a new one is created.
/// 
/// # Arguments
/// 
/// * `task_id` - The ID of the task to get a waker for
/// 
/// # Returns
/// 
/// A reference to the waker for the specified task
pub fn get_task_waker(task_id: usize) -> &'static Waker {
    let wakers_mutex = TASK_WAKERS.call_once(init_task_wakers);
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

/// Get or create a parent waker for waitpid(-1) operations
/// 
/// This waker is used when a parent process calls waitpid(-1) to wait for any child.
/// It's separate from the task-specific wakers to avoid conflicts.
/// 
/// # Arguments
/// 
/// * `parent_id` - The ID of the parent task
/// 
/// # Returns
/// 
/// A reference to the parent waker
pub fn get_parent_waker(parent_id: usize) -> &'static Waker {
    let wakers_mutex = PARENT_WAKERS.call_once(init_parent_wakers);
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
    let wakers_mutex = TASK_WAKERS.call_once(init_task_wakers);
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
    let wakers_mutex = PARENT_WAKERS.call_once(init_parent_wakers);
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
    let wakers_mutex = TASK_WAKERS.call_once(init_task_wakers);
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
    let wakers_mutex = PARENT_WAKERS.call_once(init_parent_wakers);
    let mut wakers = wakers_mutex.lock();
    wakers.remove(&parent_id);
}

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

pub struct Task {
    id: usize,
    pub name: String,
    pub priority: u32,
    pub vcpu: Vcpu,
    pub state: TaskState,
    pub task_type: TaskType,
    pub entry: usize,
    pub brk: Option<usize>, /* Program break (NOT work in Kernel task) */
    pub stack_size: usize, /* Size of the stack in bytes */
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
    parent_id: Option<usize>,      /* Parent task ID */
    children: Vec<usize>,          /* List of child task IDs */
    exit_status: Option<i32>,      /* Exit code (for monitoring child task termination) */

    /// Dynamic ABI
    pub abi: Option<Box<dyn AbiModule>>,

    // Current working directory
    pub cwd: Option<String>,

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
}

#[derive(Debug, Clone)]
pub struct ManagedPage {
    pub vaddr: usize,
    pub page: Box<Page>,
}

pub enum CloneFlagsDef {
    Vm      = 0b00000001, // Clone the VM
    Fs      = 0b00000010, // Clone the filesystem
    Files   = 0b00000100, // Clone the file descriptors
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
            abi: Some(Box::new(ScarletAbi::default())), // Default ABI
            cwd: None,
            vfs: None,
            handle_table: HandleTable::new(),
        };

        *taskid += 1;
        task
    }
    
    pub fn init(&mut self) {
        match self.task_type {
            TaskType::Kernel => {
                user_kernel_vm_init(self);
                /* Set sp to the top of the kernel stack */
                self.vcpu.set_sp(KERNEL_VM_STACK_END + 1);

            },
            TaskType::User => { 
                user_vm_init(self);
                /* Set sp to the top of the user stack */
                self.vcpu.set_sp(0xffff_ffff_ffff_f000);
            }
        }
        
        /* Set the task state to Ready */
        self.state = TaskState::Ready;
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
        if self.brk.is_none() {
            return self.text_size + self.data_size;
        }
        self.brk.unwrap()
    }

    /// Set the program break (NOT work in Kernel task)
    /// 
    /// # Arguments
    /// * `brk` - The new program break address
    /// 
    /// # Returns
    /// If successful, returns Ok(()), otherwise returns an error.
    pub fn set_brk(&mut self, brk: usize) -> Result<(), &'static str> {
        // println!("New brk: {:#x}", brk);
        if brk < self.text_size {
            return Err("Invalid address");
        }
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

            if num_of_pages > 0 {
                match self.vm_manager.search_memory_map(prev_addr) {
                    Some(_) => {},
                    None => {
                        match self.allocate_data_pages(prev_addr, num_of_pages) {
                            Ok(_) => {},
                            Err(_) => return Err("Failed to allocate pages"),
                        }
                    },
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
    pub fn allocate_pages(&mut self, vaddr: usize, num_of_pages: usize, permissions: usize) -> Result<VirtualMemoryMap, &'static str> {

        if vaddr % PAGE_SIZE != 0 {
            return Err("Address is not page aligned");
        }
        
        let pages = allocate_raw_pages(num_of_pages);
        let size = num_of_pages * PAGE_SIZE;
        let paddr = pages as usize;
        let mmap = VirtualMemoryMap {
            pmarea: MemoryArea {
                start: paddr,
                end: paddr + size - 1,
            },
            vmarea: MemoryArea {
                start: vaddr,
                end: vaddr + size - 1,
            },
            permissions,
            is_shared: false, // Default to not shared for task-allocated pages
        };
        self.vm_manager.add_memory_map(mmap).map_err(|e| panic!("Failed to add memory map: {}", e))?;

        for i in 0..num_of_pages {
            let page = unsafe { Box::from_raw(pages.wrapping_add(i)) };
            let vaddr = mmap.vmarea.start + i * PAGE_SIZE;
            self.add_managed_page(ManagedPage {
                vaddr,
                page
            });
        }


        Ok(mmap)
    }

    /// Free pages for the task.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    pub fn free_pages(&mut self, vaddr: usize, num_of_pages: usize) {
        let page = vaddr / PAGE_SIZE;
        for p in 0..num_of_pages {
            let vaddr = (page + p) * PAGE_SIZE;
            match self.vm_manager.search_memory_map_idx(vaddr) {
                Some(idx) => {
                    let mmap = self.vm_manager.remove_memory_map(idx).unwrap();
                    if p == 0 && mmap.vmarea.start < vaddr {
                        /* Re add the first part of the memory map */
                        let size = vaddr - mmap.vmarea.start;
                        let paddr = mmap.pmarea.start;
                        let mmap1 = VirtualMemoryMap {
                            pmarea: MemoryArea {
                                start: paddr,
                                end: paddr + size - 1,
                            },
                            vmarea: MemoryArea {
                                start: mmap.vmarea.start,
                                end: vaddr - 1,
                            },
                            permissions: mmap.permissions,
                            is_shared: mmap.is_shared,
                        };
                        self.vm_manager.add_memory_map(mmap1)
                            .map_err(|e| panic!("Failed to add memory map: {}", e)).unwrap();
                        // println!("Removed map : {:#x} - {:#x}", mmap.vmarea.start, mmap.vmarea.end);
                        // println!("Re added map: {:#x} - {:#x}", mmap1.vmarea.start, mmap1.vmarea.end);
                    }
                    if p == num_of_pages - 1 && mmap.vmarea.end > vaddr + PAGE_SIZE - 1 {
                        /* Re add the second part of the memory map */
                        let size = mmap.vmarea.end - (vaddr + PAGE_SIZE) + 1;
                        let paddr = mmap.pmarea.start + (vaddr + PAGE_SIZE - mmap.vmarea.start);
                        let mmap2 = VirtualMemoryMap {
                            pmarea: MemoryArea {
                                start: paddr,
                                end: paddr + size - 1,
                            },
                            vmarea: MemoryArea {
                                start: vaddr + PAGE_SIZE,
                                end: mmap.vmarea.end,
                            },
                            permissions: mmap.permissions,
                            is_shared: mmap.is_shared,
                        };
                        self.vm_manager.add_memory_map(mmap2)
                            .map_err(|e| panic!("Failed to add memory map: {}", e)).unwrap();
                        // println!("Removed map : {:#x} - {:#x}", mmap.vmarea.start, mmap.vmarea.end);
                        // println!("Re added map: {:#x} - {:#x}", mmap2.vmarea.start, mmap2.vmarea.end);
                    }
                    // let offset = vaddr - mmap.vmarea.start;
                    // free_raw_pages((mmap.pmarea.start + offset) as *mut Page, 1);

                    if let Some(free_page) = self.remove_managed_page(vaddr) {
                        free_boxed_page(free_page);
                    }
                    
                    // println!("Freed pages : {:#x} - {:#x}", vaddr, vaddr + PAGE_SIZE - 1);
                },
                None => {},
            }
        }
        /* Unmap pages */
        let root_pagetable = self.vm_manager.get_root_page_table().unwrap();
        for p in 0..num_of_pages {
            let vaddr = (page + p) * PAGE_SIZE;
            root_pagetable.unmap(vaddr);
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
    pub fn allocate_text_pages(&mut self, vaddr: usize, num_of_pages: usize) -> Result<VirtualMemoryMap, &'static str> {
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
    pub fn allocate_stack_pages(&mut self, vaddr: usize, num_of_pages: usize) -> Result<VirtualMemoryMap, &'static str> {
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
    pub fn allocate_data_pages(&mut self, vaddr: usize, num_of_pages: usize) -> Result<VirtualMemoryMap, &'static str> {
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
    pub fn allocate_guard_pages(&mut self, vaddr: usize, num_of_pages: usize) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Guard.default_permissions();
        let mmap = VirtualMemoryMap {
            pmarea: MemoryArea {
                start: 0,
                end: 0,
            },
            vmarea: MemoryArea {
                start: vaddr,
                end: vaddr + num_of_pages * PAGE_SIZE - 1,
            },
            permissions,
            is_shared: VirtualMemoryRegion::Guard.is_shareable(), // Guard pages can be shared
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
    fn remove_managed_page(&mut self, vaddr: usize) -> Option<Box<Page>> {
        for i in 0..self.managed_pages.len() {
            if self.managed_pages[i].vaddr == vaddr {
                let page = self.managed_pages.remove(i);
                return Some(page.page);
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
        let mut child = Task::new(
            self.name.clone(),
            self.priority,
            self.task_type
        );
        
        // First, set up the virtual memory manager with the same ASID allocation
        match self.task_type {
            TaskType::Kernel => {
                // For kernel tasks, we need to call init to set up the kernel VM
                child.init();
            },
            TaskType::User => {
                if !flags.is_set(CloneFlagsDef::Vm) {
                    // For user tasks, manually set up VM without calling init()
                    // to avoid creating new stack that would overwrite parent's stack content
                    let asid = alloc_virtual_address_space();
                    child.vm_manager.set_asid(asid);
                }
            }
        }
        
        if !flags.is_set(CloneFlagsDef::Vm) {
            // Copy or share memory maps from parent to child
            for mmap in self.vm_manager.get_memmap() {
                let num_pages = (mmap.vmarea.end - mmap.vmarea.start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
                let vaddr = mmap.vmarea.start;
                
                if num_pages > 0 {
                    if mmap.is_shared {
                        // Shared memory regions: just reference the same physical pages
                        let shared_mmap = VirtualMemoryMap {
                            pmarea: mmap.pmarea, // Same physical memory
                            vmarea: mmap.vmarea, // Same virtual addresses
                            permissions: mmap.permissions,
                            is_shared: true,
                        };
                        // Add the shared memory map directly to the child task
                        child.vm_manager.add_memory_map(shared_mmap)
                            .map_err(|_| "Failed to add shared memory map to child task")?;

                        // TODO: Add logic to determine if the memory map is a trampoline
                        // If the memory map is the trampoline, pre-map it
                        if mmap.vmarea.start == 0xffff_ffff_ffff_f000 {
                            // Pre-map the trampoline page
                            let root_pagetable = child.vm_manager.get_root_page_table().unwrap();
                            root_pagetable.map_memory_area(child.vm_manager.get_asid(), shared_mmap)?;
                        }

                    } else {
                        // Private memory regions: allocate new pages and copy contents
                        let permissions = mmap.permissions;
                        let pages = allocate_raw_pages(num_pages);
                        let size = num_pages * PAGE_SIZE;
                        let paddr = pages as usize;
                        let new_mmap = VirtualMemoryMap {
                            pmarea: MemoryArea {
                                start: paddr,
                                end: paddr + (size - 1),
                            },
                            vmarea: MemoryArea {
                                start: vaddr,
                                end: vaddr + (size - 1),
                            },
                            permissions,
                            is_shared: false,
                        };
                        
                        // Copy the contents of the original memory (including stack contents)
                        for i in 0..num_pages {
                            let src_page_addr = mmap.pmarea.start + i * PAGE_SIZE;
                            let dst_page_addr = new_mmap.pmarea.start + i * PAGE_SIZE;
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    src_page_addr as *const u8,
                                    dst_page_addr as *mut u8,
                                    PAGE_SIZE
                                );
                            }
                            // Manage the new pages in the child task
                            child.add_managed_page(ManagedPage {
                                vaddr: new_mmap.vmarea.start + i * PAGE_SIZE,
                                page: unsafe { Box::from_raw(pages.wrapping_add(i)) },
                            });
                        }
                        // Add the new memory map to the child task
                        child.vm_manager.add_memory_map(new_mmap)
                            .map_err(|_| "Failed to add memory map to child task")?;
                    }
                }
            }
        }

        // Copy register states
        child.vcpu.regs = self.vcpu.regs.clone();
        
        // Set the ABI
        if let Some(abi) = &self.abi {
            child.abi = Some(abi.clone_boxed());
        } else {
            child.abi = None; // No ABI set
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
                // Copy the current working directory
                child.cwd = self.cwd.clone();
            } else {
                child.vfs = None;
                child.cwd = None; // No filesystem manager, no current working directory
            }
        }

        // Set the ABI
        if let Some(abi) = &self.abi {
            child.abi = Some(abi.clone_boxed());
        } else {
            child.abi = None; // No ABI set
        }

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
        // crate::println!("Task {} ({}) exiting with status {}", self.id, self.name, status);
        
        // Close all open handles when task exits
        self.handle_table.close_all();
        
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
                // crate::println!("Task {}: Set to Zombie state, parent {}", self.id, parent_id);
            },
            None => {
                /* If the task has no parent, it is terminated */
                // crate::println!("Task {}: No parent, terminating", self.id);
                self.state = TaskState::Terminated;
            }
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
            return Err(WaitError::NoSuchChild("No such child task".to_string()));
        }

        if let Some(child_task) = get_scheduler().get_task_by_id(child_id) {
            if child_task.get_state() == TaskState::Zombie {
                let status = child_task.get_exit_status().unwrap_or(-1);
                child_task.set_state(TaskState::Terminated);
                self.remove_child(child_id);
                Ok(status)
            } else {
                Err(WaitError::ChildNotExited("Child has not exited or is not a zombie".to_string()))
            }
        } else {
            Err(WaitError::ChildTaskNotFound("Child task not found".to_string()))
        }
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

    /// Set the current working directory
    pub fn set_cwd(&mut self, cwd: String) {
        self.cwd = Some(cwd);
    }

    /// Get the current working directory
    pub fn get_cwd(&self) -> Option<&String> {
        self.cwd.as_ref()
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

/// Get the current task.
/// 
/// # Returns
/// The current task if it exists.
pub fn mytask() -> Option<&'static mut Task> {
    let cpu = get_cpu();
    get_scheduler().get_current_task(cpu.get_cpuid())
}

/// Set the current working directory for the current task
/// 
/// # Arguments
/// * `cwd` - New current working directory path
/// 
/// # Returns
/// * `true` if successful, `false` if no current task
pub fn set_current_task_cwd(cwd: String) -> bool {
    if let Some(task) = mytask() {
        task.set_cwd(cwd);
        true
    } else {
        false
    }
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
        let mmap = parent_task.allocate_data_pages(vaddr, num_pages).unwrap();

        // Write test data to parent's memory
        let test_data: [u8; 8] = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        unsafe {
            let dst_ptr = mmap.pmarea.start as *mut u8;
            core::ptr::copy_nonoverlapping(test_data.as_ptr(), dst_ptr, test_data.len());
        }

        // Get parent memory map count before cloning
        let parent_memmap_count = parent_task.vm_manager.get_memmap().len();
        let parent_id = parent_task.get_id();

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // Get child memory map count after cloning
        let child_memmap_count = child_task.vm_manager.get_memmap().len();

        // Verify that the number of memory maps are identical
        assert_eq!(child_memmap_count, parent_memmap_count, 
            "Child should have the same number of memory maps as parent: child={}, parent={}",
            child_memmap_count, parent_memmap_count);

        // Verify parent-child relationship was established
        assert_eq!(child_task.get_parent_id(), Some(parent_id));
        assert!(parent_task.get_children().contains(&child_task.get_id()));

        // Verify memory sizes were copied
        assert_eq!(child_task.stack_size, parent_task.stack_size);
        assert_eq!(child_task.data_size, parent_task.data_size);
        assert_eq!(child_task.text_size, parent_task.text_size);

        // Find the corresponding memory map in child that matches our test allocation
        let child_memmaps = child_task.vm_manager.get_memmap();
        let child_mmap = child_memmaps.iter()
            .find(|mmap| mmap.vmarea.start == vaddr && mmap.vmarea.end == vaddr + num_pages * crate::environment::PAGE_SIZE - 1)
            .expect("Test memory map not found in child task");

        // Verify that our specific memory region exists in both parent and child
        let parent_memmaps = parent_task.vm_manager.get_memmap();
        let parent_test_mmap = parent_memmaps.iter()
            .find(|mmap| mmap.vmarea.start == vaddr && mmap.vmarea.end == vaddr + num_pages * crate::environment::PAGE_SIZE - 1)
            .expect("Test memory map not found in parent task");

        // Verify the virtual memory ranges match
        assert_eq!(child_mmap.vmarea.start, parent_test_mmap.vmarea.start);
        assert_eq!(child_mmap.vmarea.end, parent_test_mmap.vmarea.end);
        assert_eq!(child_mmap.permissions, parent_test_mmap.permissions);

        // Verify the data was copied correctly
        unsafe {
            let parent_ptr = mmap.pmarea.start as *const u8;
            let child_ptr = child_mmap.pmarea.start as *const u8;
            
            // Check that physical addresses are different (separate memory)
            assert_ne!(parent_ptr, child_ptr, "Parent and child should have different physical memory");
            
            // Check that the data content is identical
            for i in 0..test_data.len() {
                let parent_byte = *parent_ptr.offset(i as isize);
                let child_byte = *child_ptr.offset(i as isize);
                assert_eq!(parent_byte, child_byte, "Data mismatch at offset {}", i);
            }
        }

        // Verify that modifying parent's memory doesn't affect child's memory
        unsafe {
            let parent_ptr = mmap.pmarea.start as *mut u8;
            let original_value = *parent_ptr;
            *parent_ptr = 0xFF; // Modify first byte in parent
            
            let child_ptr = child_mmap.pmarea.start as *const u8;
            let child_first_byte = *child_ptr;
            
            // Child's first byte should still be the original value
            assert_eq!(child_first_byte, original_value, "Child memory should be independent from parent");
        }

        // Verify register states were copied
        assert_eq!(child_task.vcpu.get_pc(), parent_task.vcpu.get_pc());
        
        // Verify entry point was copied
        assert_eq!(child_task.entry, parent_task.entry);

        // Verify state was copied
        assert_eq!(child_task.state, parent_task.state);

        // Verify that both tasks have the correct number of managed pages
        assert!(child_task.managed_pages.len() >= num_pages, 
            "Child should have at least the test pages in managed pages");
    }

    #[test_case]
    fn test_clone_task_stack_copy() {
        let mut parent_task = super::new_user_task("ParentWithStack".to_string(), 0);
        parent_task.init();

        // Find the stack memory map in parent
        let stack_mmap = parent_task.vm_manager.get_memmap().iter()
            .find(|mmap| {
                // Stack should be near USER_STACK_TOP and have stack permissions
                use crate::vm::vmem::VirtualMemoryRegion;
                mmap.vmarea.end == crate::environment::USER_STACK_TOP - 1 && 
                mmap.permissions == VirtualMemoryRegion::Stack.default_permissions()
            })
            .expect("Stack memory map not found in parent task")
            .clone();

        // Write test data to parent's stack
        let stack_test_data: [u8; 16] = [
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22,
            0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00
        ];
        unsafe {
            let stack_ptr = (stack_mmap.pmarea.start + crate::environment::PAGE_SIZE) as *mut u8;
            core::ptr::copy_nonoverlapping(stack_test_data.as_ptr(), stack_ptr, stack_test_data.len());
        }

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // Find the corresponding stack memory map in child
        let child_stack_mmap = child_task.vm_manager.get_memmap().iter()
            .find(|mmap| {
                use crate::vm::vmem::VirtualMemoryRegion;
                mmap.vmarea.start == stack_mmap.vmarea.start &&
                mmap.vmarea.end == stack_mmap.vmarea.end &&
                mmap.permissions == VirtualMemoryRegion::Stack.default_permissions()
            })
            .expect("Stack memory map not found in child task");

        // Verify that stack content was copied correctly
        unsafe {
            let parent_stack_ptr = (stack_mmap.pmarea.start + crate::environment::PAGE_SIZE) as *const u8;
            let child_stack_ptr = (child_stack_mmap.pmarea.start + crate::environment::PAGE_SIZE) as *const u8;

            // Check that physical addresses are different (separate memory)
            assert_ne!(parent_stack_ptr, child_stack_ptr, 
                "Parent and child should have different stack physical memory");

            // Check that the stack data content is identical
            for i in 0..stack_test_data.len() {
                let parent_byte = *parent_stack_ptr.offset(i as isize);
                let child_byte = *child_stack_ptr.offset(i as isize);
                assert_eq!(parent_byte, child_byte, 
                    "Stack data mismatch at offset {}: parent={:#x}, child={:#x}", 
                    i, parent_byte, child_byte);
            }
        }

        // Verify that modifying parent's stack doesn't affect child's stack
        unsafe {
            let parent_stack_ptr = (stack_mmap.pmarea.start + crate::environment::PAGE_SIZE) as *mut u8;
            let original_value = *parent_stack_ptr;
            *parent_stack_ptr = 0xFE; // Modify first byte in parent stack

            let child_stack_ptr = (child_stack_mmap.pmarea.start + crate::environment::PAGE_SIZE) as *const u8;
            let child_first_byte = *child_stack_ptr;

            // Child's first byte should still be the original value
            assert_eq!(child_first_byte, original_value, 
                "Child stack should be independent from parent stack");
        }

        // Verify stack sizes match
        assert_eq!(child_task.stack_size, parent_task.stack_size,
            "Child and parent should have the same stack size");
    }

    #[test_case]
    fn test_clone_task_shared_memory() {
        use crate::vm::vmem::{VirtualMemoryMap, MemoryArea, VirtualMemoryPermission};
        use crate::mem::page::allocate_raw_pages;
        use crate::environment::PAGE_SIZE;
        
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
            permissions: VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            is_shared: true, // This should be shared between parent and child
        };
        
        // Add shared memory map to parent
        parent_task.vm_manager.add_memory_map(shared_mmap).unwrap();
        
        // Write test data to shared memory
        let test_data: [u8; 8] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22];
        unsafe {
            let shared_ptr = paddr as *mut u8;
            core::ptr::copy_nonoverlapping(test_data.as_ptr(), shared_ptr, test_data.len());
        }

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // Find the shared memory map in child
        let child_shared_mmap = child_task.vm_manager.get_memmap().iter()
            .find(|mmap| mmap.vmarea.start == shared_vaddr && mmap.is_shared)
            .expect("Shared memory map not found in child task");

        // Verify that the physical addresses are the same (shared memory)
        assert_eq!(child_shared_mmap.pmarea.start, shared_mmap.pmarea.start,
            "Shared memory should have the same physical address in parent and child");
        
        // Verify that the virtual addresses are the same
        assert_eq!(child_shared_mmap.vmarea.start, shared_mmap.vmarea.start);
        assert_eq!(child_shared_mmap.vmarea.end, shared_mmap.vmarea.end);
        
        // Verify that is_shared flag is preserved
        assert!(child_shared_mmap.is_shared, "Shared memory should remain marked as shared");

        // Verify that modifying shared memory from child affects parent
        unsafe {
            let child_shared_ptr = child_shared_mmap.pmarea.start as *mut u8;
            let original_value = *child_shared_ptr;
            *child_shared_ptr = 0xFF; // Modify first byte through child reference
            
            let parent_shared_ptr = shared_mmap.pmarea.start as *const u8;
            let parent_first_byte = *parent_shared_ptr;
            
            // Parent should see the change made by child (shared memory)
            assert_eq!(parent_first_byte, 0xFF, 
                "Parent should see changes made through child's shared memory reference");
                
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
                assert_eq!(parent_byte, child_byte, 
                    "Shared memory data should be identical from both parent and child views");
            }
        }
    }
}