use crate::arch::Trapframe;
use crate::environment::{PAGE_SIZE, USER_STACK_END};
use crate::task::Task;

/// Simplified process environment block data used by Windows ABI bootstrap.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PebData {
    pub image_base_address: u64,
    pub process_heap: u64,
    pub loader_data: u64,
    pub process_parameters: u64,
    pub os_major_version: u32,
    pub os_minor_version: u32,
    pub os_build_number: u16,
}

/// Simplified thread environment block data used by Windows ABI bootstrap.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TebData {
    pub self_ptr: u64,
    pub process_environment_block: u64,
    pub thread_id: u32,
    pub stack_base: u64,
    pub stack_limit: u64,
    pub tls_pointer: u64,
    pub last_error_value: u32,
}

/// Result of PEB/TEB initialization.
#[derive(Clone, Copy, Default)]
pub struct ProcessEnvironment {
    pub peb_address: u64,
    pub teb_address: u64,
}

/// Initialize simplified process environment for Windows user mode.
pub fn initialize_process_environment(
    task: &Task,
    trapframe: &mut Trapframe,
    image_base: u64,
    process_heap: u64,
) -> Result<ProcessEnvironment, &'static str> {
    let peb_addr = task
        .vm_manager
        .find_unmapped_area(PAGE_SIZE, PAGE_SIZE)
        .ok_or("No free address for PEB")?;
    let teb_addr = task
        .vm_manager
        .find_unmapped_area(PAGE_SIZE, PAGE_SIZE)
        .ok_or("No free address for TEB")?;

    task.allocate_data_pages(peb_addr, 1)?;
    task.allocate_data_pages(teb_addr, 1)?;

    let peb = PebData {
        image_base_address: image_base,
        process_heap,
        loader_data: 0,
        process_parameters: 0,
        os_major_version: 10,
        os_minor_version: 0,
        os_build_number: 22621,
    };

    let teb = TebData {
        self_ptr: teb_addr as u64,
        process_environment_block: peb_addr as u64,
        thread_id: task.get_id() as u32,
        stack_base: USER_STACK_END as u64,
        stack_limit: (USER_STACK_END - (PAGE_SIZE * 256)) as u64,
        tls_pointer: 0,
        last_error_value: 0,
    };

    write_struct(task, peb_addr, &peb)?;
    write_struct(task, teb_addr, &teb)?;

    trapframe.tpidr_el0 = teb_addr as u64;

    Ok(ProcessEnvironment {
        peb_address: peb_addr as u64,
        teb_address: teb_addr as u64,
    })
}

fn write_struct<T>(task: &Task, user_addr: usize, value: &T) -> Result<(), &'static str> {
    let kva = task
        .vm_manager
        .translate_to_kva(user_addr)
        .ok_or("Failed to translate user address")?;
    let size = core::mem::size_of::<T>();
    unsafe {
        // SAFETY: `kva` is a kernel-mapped address for a freshly allocated user page,
        // and the destination range has at least `size_of::<T>()` bytes.
        core::ptr::copy_nonoverlapping(value as *const T as *const u8, kva as *mut u8, size);
    }
    Ok(())
}
