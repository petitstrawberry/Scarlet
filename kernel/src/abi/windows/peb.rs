use crate::environment::{PAGE_SIZE, USER_STACK_END};
use crate::task::Task;

pub const SHARED_USER_DATA_BASE: usize = 0x7FFE_0000;

pub mod layout {
    pub const PEB_SIZE: usize = 0x300;
    pub const TEB_SIZE: usize = 0x800;
    pub const PEB_LDR_DATA_SIZE: usize = 0x80;
    pub const LDR_DATA_TABLE_ENTRY_SIZE: usize = 0xA0;
    pub const CONTEXT_ARM64_SIZE: usize = 0x390;

    pub const PEB_OFFSET_LDR: usize = 0x18;
    pub const PEB_OFFSET_PROCESS_PARAMETERS: usize = 0x20;
    pub const PEB_OFFSET_PROCESS_HEAP: usize = 0x30;
    pub const PEB_OFFSET_OS_MAJOR: usize = 0x118;
    pub const PEB_OFFSET_OS_MINOR: usize = 0x11C;
    pub const PEB_OFFSET_OS_BUILD: usize = 0x120;

    pub const TEB_OFFSET_PROCESS_ENVIRONMENT_BLOCK: usize = 0x60;
    pub const TEB_OFFSET_CLIENT_ID: usize = 0x40;
    pub const TEB_OFFSET_TIB_STACK_BASE: usize = 0x08;
    pub const TEB_OFFSET_TIB_STACK_LIMIT: usize = 0x10;
    pub const TEB_OFFSET_LAST_ERROR_VALUE: usize = 0x68;
    pub const TEB_OFFSET_SELF: usize = 0x30;

    pub const PEB_LDR_OFFSET_LENGTH: usize = 0x00;
    pub const PEB_LDR_OFFSET_INITIALIZED: usize = 0x04;
    pub const PEB_LDR_OFFSET_SS_HANDLE: usize = 0x08;
    pub const PEB_LDR_OFFSET_IN_LOAD_ORDER: usize = 0x10;
    pub const PEB_LDR_OFFSET_IN_MEMORY_ORDER: usize = 0x20;
    pub const PEB_LDR_OFFSET_IN_INIT_ORDER: usize = 0x30;

    pub const LDR_ENTRY_OFFSET_IN_LOAD_ORDER_LINKS: usize = 0x00;
    pub const LDR_ENTRY_OFFSET_IN_MEMORY_ORDER_LINKS: usize = 0x10;
    pub const LDR_ENTRY_OFFSET_IN_INIT_ORDER_LINKS: usize = 0x20;
    pub const LDR_ENTRY_OFFSET_DLL_BASE: usize = 0x30;
    pub const LDR_ENTRY_OFFSET_ENTRY_POINT: usize = 0x38;
    pub const LDR_ENTRY_OFFSET_SIZE_OF_IMAGE: usize = 0x40;
    pub const LDR_ENTRY_OFFSET_FULL_DLL_NAME: usize = 0x48;
    pub const LDR_ENTRY_OFFSET_BASE_DLL_NAME: usize = 0x58;
    pub const LDR_ENTRY_OFFSET_BASE_NAME_HASH_VALUE: usize = 0x80;
    pub const LDR_ENTRY_OFFSET_LOAD_REASON: usize = 0x84;
    pub const LDR_ENTRY_OFFSET_LOAD_COUNT: usize = 0x88;
}

#[derive(Clone, Copy, Default)]
pub struct NtDllData {
    pub ldr_data_address: u64,
    pub image_entry_address: u64,
    pub ntdll_entry_address: u64,
}

#[derive(Clone, Copy, Default)]
pub struct ProcessEnvironment {
    pub peb_address: u64,
    pub teb_address: u64,
    pub context_address: u64,
    pub context_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ListEntry {
    pub flink: u64,
    pub blink: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ClientId {
    pub unique_process: u64,
    pub unique_thread: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    pub _pad: u32,
    pub buffer: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NtdllContext {
    pub context_flags: u32,
    pub cpsr: u32,
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
}

impl NtdllContext {
    pub const CONTEXT_CONTROL: u32 = 0x0000_0001;
    pub const CONTEXT_INTEGER: u32 = 0x0000_0002;
    pub const CONTEXT_FULL: u32 = Self::CONTEXT_CONTROL | Self::CONTEXT_INTEGER;
}

pub fn initialize_process_environment(
    task: &Task,
    image_base: u64,
    image_entry: u64,
    image_size: u64,
    process_heap: u64,
    ntdll: NtDllData,
    ldr_initialize_thunk: u64,
    initial_stack_pointer: u64,
) -> Result<ProcessEnvironment, &'static str> {
    map_shared_user_data(task)?;

    let teb_addr = task
        .vm_manager
        .find_unmapped_area(PAGE_SIZE, PAGE_SIZE)
        .ok_or("No free address for TEB")?;
    let peb_addr = task
        .vm_manager
        .find_unmapped_area(PAGE_SIZE, PAGE_SIZE)
        .ok_or("No free address for PEB")?;
    let ldr_addr = task
        .vm_manager
        .find_unmapped_area(PAGE_SIZE, PAGE_SIZE)
        .ok_or("No free address for PEB_LDR_DATA")?;
    let exe_entry_addr = task
        .vm_manager
        .find_unmapped_area(PAGE_SIZE, PAGE_SIZE)
        .ok_or("No free address for executable LDR entry")?;
    let ntdll_entry_addr = task
        .vm_manager
        .find_unmapped_area(PAGE_SIZE, PAGE_SIZE)
        .ok_or("No free address for ntdll LDR entry")?;

    task.allocate_data_pages(teb_addr, 1)?;
    task.allocate_data_pages(peb_addr, 1)?;
    task.allocate_data_pages(ldr_addr, 1)?;
    task.allocate_data_pages(exe_entry_addr, 1)?;
    task.allocate_data_pages(ntdll_entry_addr, 1)?;

    let mut teb_buf = [0u8; PAGE_SIZE];
    let mut peb_buf = [0u8; PAGE_SIZE];
    let mut ldr_buf = [0u8; PAGE_SIZE];
    let mut exe_entry_buf = [0u8; PAGE_SIZE];
    let mut ntdll_entry_buf = [0u8; PAGE_SIZE];

    write_u64(
        &mut teb_buf,
        layout::TEB_OFFSET_PROCESS_ENVIRONMENT_BLOCK,
        peb_addr as u64,
    );
    write_u64(&mut teb_buf, layout::TEB_OFFSET_SELF, teb_addr as u64);
    write_u64(
        &mut teb_buf,
        layout::TEB_OFFSET_TIB_STACK_BASE,
        USER_STACK_END as u64,
    );
    write_u64(
        &mut teb_buf,
        layout::TEB_OFFSET_TIB_STACK_LIMIT,
        (USER_STACK_END - PAGE_SIZE * 256) as u64,
    );
    write_u64(
        &mut teb_buf,
        layout::TEB_OFFSET_CLIENT_ID,
        task.get_id() as u64,
    );
    write_u64(
        &mut teb_buf,
        layout::TEB_OFFSET_CLIENT_ID + 8,
        task.get_id() as u64,
    );
    write_u32(&mut teb_buf, layout::TEB_OFFSET_LAST_ERROR_VALUE, 0);

    write_u64(&mut peb_buf, layout::PEB_OFFSET_LDR, ldr_addr as u64);
    write_u64(&mut peb_buf, layout::PEB_OFFSET_PROCESS_HEAP, process_heap);
    write_u64(&mut peb_buf, layout::PEB_OFFSET_PROCESS_PARAMETERS, 0);
    write_u64(&mut peb_buf, 0x10, image_base);
    write_u32(&mut peb_buf, layout::PEB_OFFSET_OS_MAJOR, 10);
    write_u32(&mut peb_buf, layout::PEB_OFFSET_OS_MINOR, 0);
    write_u16(&mut peb_buf, layout::PEB_OFFSET_OS_BUILD, 22621);

    let load_head = (ldr_addr + layout::PEB_LDR_OFFSET_IN_LOAD_ORDER) as u64;
    let mem_head = (ldr_addr + layout::PEB_LDR_OFFSET_IN_MEMORY_ORDER) as u64;
    let init_head = (ldr_addr + layout::PEB_LDR_OFFSET_IN_INIT_ORDER) as u64;

    let exe_load = (exe_entry_addr + layout::LDR_ENTRY_OFFSET_IN_LOAD_ORDER_LINKS) as u64;
    let exe_mem = (exe_entry_addr + layout::LDR_ENTRY_OFFSET_IN_MEMORY_ORDER_LINKS) as u64;
    let exe_init = (exe_entry_addr + layout::LDR_ENTRY_OFFSET_IN_INIT_ORDER_LINKS) as u64;
    let ntdll_load = (ntdll_entry_addr + layout::LDR_ENTRY_OFFSET_IN_LOAD_ORDER_LINKS) as u64;
    let ntdll_mem = (ntdll_entry_addr + layout::LDR_ENTRY_OFFSET_IN_MEMORY_ORDER_LINKS) as u64;
    let ntdll_init = (ntdll_entry_addr + layout::LDR_ENTRY_OFFSET_IN_INIT_ORDER_LINKS) as u64;

    write_u32(
        &mut ldr_buf,
        layout::PEB_LDR_OFFSET_LENGTH,
        layout::PEB_LDR_DATA_SIZE as u32,
    );
    write_u8(&mut ldr_buf, layout::PEB_LDR_OFFSET_INITIALIZED, 1);
    write_list_entry(
        &mut ldr_buf,
        layout::PEB_LDR_OFFSET_IN_LOAD_ORDER,
        exe_load,
        ntdll_load,
    );
    write_list_entry(
        &mut ldr_buf,
        layout::PEB_LDR_OFFSET_IN_MEMORY_ORDER,
        exe_mem,
        ntdll_mem,
    );
    write_list_entry(
        &mut ldr_buf,
        layout::PEB_LDR_OFFSET_IN_INIT_ORDER,
        exe_init,
        ntdll_init,
    );

    write_list_entry(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_IN_LOAD_ORDER_LINKS,
        ntdll_load,
        load_head,
    );
    write_list_entry(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_IN_MEMORY_ORDER_LINKS,
        ntdll_mem,
        mem_head,
    );
    write_list_entry(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_IN_INIT_ORDER_LINKS,
        ntdll_init,
        init_head,
    );
    write_u64(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_DLL_BASE,
        image_base,
    );
    write_u64(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_ENTRY_POINT,
        image_entry,
    );
    write_u32(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_SIZE_OF_IMAGE,
        image_size as u32,
    );
    write_u32(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_BASE_NAME_HASH_VALUE,
        0xF6B1_95A2,
    );
    write_u32(&mut exe_entry_buf, layout::LDR_ENTRY_OFFSET_LOAD_REASON, 0);
    write_u16(&mut exe_entry_buf, layout::LDR_ENTRY_OFFSET_LOAD_COUNT, 1);

    write_list_entry(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_IN_LOAD_ORDER_LINKS,
        load_head,
        exe_load,
    );
    write_list_entry(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_IN_MEMORY_ORDER_LINKS,
        mem_head,
        exe_mem,
    );
    write_list_entry(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_IN_INIT_ORDER_LINKS,
        init_head,
        exe_init,
    );
    write_u64(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_DLL_BASE,
        ntdll.ntdll_entry_address,
    );
    write_u64(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_ENTRY_POINT,
        ntdll.ntdll_entry_address,
    );
    write_u32(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_SIZE_OF_IMAGE,
        0x0020_0000,
    );
    write_u32(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_BASE_NAME_HASH_VALUE,
        0x841C_5859,
    );
    write_u32(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_LOAD_REASON,
        0,
    );
    write_u16(&mut ntdll_entry_buf, layout::LDR_ENTRY_OFFSET_LOAD_COUNT, 1);

    write_bytes(task, teb_addr, &teb_buf)?;
    write_bytes(task, peb_addr, &peb_buf)?;
    write_bytes(task, ldr_addr, &ldr_buf)?;
    write_bytes(task, exe_entry_addr, &exe_entry_buf)?;
    write_bytes(task, ntdll_entry_addr, &ntdll_entry_buf)?;

    let _ = ntdll.ldr_data_address;
    let _ = ntdll.image_entry_address;

    let ctx_addr = allocate_context_record(
        task,
        initial_stack_pointer,
        image_entry,
        ldr_initialize_thunk,
    )?;

    Ok(ProcessEnvironment {
        peb_address: peb_addr as u64,
        teb_address: teb_addr as u64,
        context_address: ctx_addr,
        context_size: layout::CONTEXT_ARM64_SIZE,
    })
}

fn map_shared_user_data(task: &Task) -> Result<(), &'static str> {
    if task
        .vm_manager
        .translate_to_kva(SHARED_USER_DATA_BASE)
        .is_none()
    {
        task.allocate_data_pages(SHARED_USER_DATA_BASE, 1)?;
        let kva = task
            .vm_manager
            .translate_to_kva(SHARED_USER_DATA_BASE)
            .ok_or("Failed to map SharedUserData")?;
        unsafe {
            // SAFETY: KVA is mapped to a freshly allocated page, writable for PAGE_SIZE bytes.
            core::ptr::write_bytes(kva as *mut u8, 0, PAGE_SIZE);
        }
        unsafe {
            // SAFETY: same mapping guarantees as above.
            *((kva + 0x026C) as *mut u32) = 10;
            *((kva + 0x0270) as *mut u32) = 0;
            *((kva + 0x0260) as *mut u16) = 22621;
        }
    }
    Ok(())
}

fn allocate_context_record(
    task: &Task,
    stack_pointer: u64,
    user_entry: u64,
    ldr_initialize_thunk: u64,
) -> Result<u64, &'static str> {
    let mut ctx_sp = (stack_pointer as usize).saturating_sub(layout::CONTEXT_ARM64_SIZE);
    ctx_sp &= !0xF;

    let context = NtdllContext {
        context_flags: NtdllContext::CONTEXT_FULL,
        cpsr: 0,
        x: [0; 31],
        sp: stack_pointer,
        pc: user_entry,
    };

    write_struct(task, ctx_sp, &context)?;
    let _ = ldr_initialize_thunk;
    Ok(ctx_sp as u64)
}

fn write_struct<T>(task: &Task, user_addr: usize, value: &T) -> Result<(), &'static str> {
    let kva = task
        .vm_manager
        .translate_to_kva(user_addr)
        .ok_or("Failed to translate user address")?;
    let size = core::mem::size_of::<T>();
    unsafe {
        // SAFETY: `kva` points to mapped user memory and destination has room for `size` bytes.
        core::ptr::copy_nonoverlapping(value as *const T as *const u8, kva as *mut u8, size);
    }
    Ok(())
}

fn write_bytes(task: &Task, user_addr: usize, value: &[u8]) -> Result<(), &'static str> {
    let kva = task
        .vm_manager
        .translate_to_kva(user_addr)
        .ok_or("Failed to translate user address")?;
    unsafe {
        // SAFETY: `kva` points to mapped user memory and destination has at least `value.len()` bytes.
        core::ptr::copy_nonoverlapping(value.as_ptr(), kva as *mut u8, value.len());
    }
    Ok(())
}

fn write_list_entry(buf: &mut [u8], offset: usize, flink: u64, blink: u64) {
    write_u64(buf, offset, flink);
    write_u64(buf, offset + 8, blink);
}

fn write_u8(buf: &mut [u8], offset: usize, value: u8) {
    buf[offset] = value;
}

fn write_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(buf: &mut [u8], offset: usize, value: u64) {
    buf[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
