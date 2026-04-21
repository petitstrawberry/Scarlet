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
    pub const PEB_OFFSET_NUMBER_OF_PROCESSORS: usize = 0xB8; // ULONG on 64-bit PEB

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
    pub ntdll_entry_point: u64,
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
    pub const CONTEXT_ARM64: u32 = 0x0040_0000;
    pub const CONTEXT_CONTROL: u32 = 0x0000_0001;
    pub const CONTEXT_INTEGER: u32 = 0x0000_0002;
    pub const CONTEXT_FLOATING_POINT: u32 = 0x0000_0004;
    pub const CONTEXT_FULL: u32 = Self::CONTEXT_ARM64
        | Self::CONTEXT_CONTROL
        | Self::CONTEXT_INTEGER
        | Self::CONTEXT_FLOATING_POINT;
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
    teb_addr: usize,
    peb_addr: usize,
    ldr_addr: usize,
    exe_entry_addr: usize,
    ntdll_entry_addr: usize,
) -> Result<ProcessEnvironment, &'static str> {
    map_shared_user_data(task)?;

    task.allocate_data_pages(teb_addr, 4)?;
    task.allocate_data_pages(peb_addr, 1)?;
    task.allocate_data_pages(ldr_addr, 1)?;
    task.allocate_data_pages(exe_entry_addr, 1)?;
    task.allocate_data_pages(ntdll_entry_addr, 1)?;

    let proc_params_addr: usize = 0x0000_0002_0004_0000;
    task.allocate_data_pages(proc_params_addr, 1)?;

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
    write_u64(
        &mut peb_buf,
        layout::PEB_OFFSET_PROCESS_PARAMETERS,
        proc_params_addr as u64,
    );

    // PEB heap config — ntdll reads these via RtlCreateHeap
    write_u64(&mut peb_buf, 0xC8, 0x100000);  // HeapSegmentReserve
    write_u64(&mut peb_buf, 0xD0, 0x2000);    // HeapSegmentCommit
    write_u64(&mut peb_buf, 0xD8, 0x10000);   // HeapDeCommitTotalFreeThreshold
    write_u64(&mut peb_buf, 0xE0, 0x1000);    // HeapDeCommitFreeBlockThreshold
    write_u32(&mut peb_buf, 0xE8, 0);          // NumberOfHeaps
    write_u32(&mut peb_buf, 0xEC, 0x100);     // MaximumNumberOfHeaps
    write_u64(&mut peb_buf, 0xF0, peb_addr as u64 + layout::PEB_SIZE as u64); // ProcessHeaps

    // Mutant handle at PEB+0x08 — must be non-NULL for loader lock
    write_u64(&mut peb_buf, 0x08, 0xFFFFFFFFFFFFFFFF);

    // ApiSetMap at PEB+0x68 — API_SET_NAMESPACE V6 (Windows 10/11)
    const API_SET_MAP_ADDR: usize = 0x0000_0002_0006_0000;
    task.allocate_data_pages(API_SET_MAP_ADDR, 1)?;
    let mut apiset_buf = [0u8; PAGE_SIZE];
    // API_SET_NAMESPACE (28 bytes, all ULONG):
    write_u32(&mut apiset_buf, 0x00, 6); // Version = API_SET_SCHEMA_VERSION_V6
    write_u32(&mut apiset_buf, 0x04, 28); // Size = header size
    write_u32(&mut apiset_buf, 0x08, 0); // Flags
    write_u32(&mut apiset_buf, 0x0C, 0); // Count = 0 (empty)
    write_u32(&mut apiset_buf, 0x10, 28); // EntryOffset (past header)
    write_u32(&mut apiset_buf, 0x14, 28); // HashOffset (past header)
    write_u32(&mut apiset_buf, 0x18, 0); // HashFactor
    write_bytes(task, API_SET_MAP_ADDR, &apiset_buf)?;
    write_u64(&mut peb_buf, 0x68, API_SET_MAP_ADDR as u64);

    let mut params_buf = [0u8; PAGE_SIZE];

    // RTL_USER_PROCESS_PARAMETERS layout (PE32+ / ARM64)
    write_u32(&mut params_buf, 0x00, PAGE_SIZE as u32); // MaximumLength
    write_u32(&mut params_buf, 0x04, PAGE_SIZE as u32); // Length
    write_u32(&mut params_buf, 0x08, 0x6001); // Flags: PPF_NORMALIZED | ?
    write_u64(&mut params_buf, 0x10, 0); // ConsoleHandle = NULL
    write_u32(&mut params_buf, 0x18, 0); // ConsoleFlags
    write_u64(&mut params_buf, 0x20, 0xFFFFFFFFFFFFFFFF); // StandardInput
    write_u64(&mut params_buf, 0x28, 0xFFFFFFFFFFFFFFFF); // StandardOutput
    write_u64(&mut params_buf, 0x30, 0xFFFFFFFFFFFFFFFF); // StandardError

    // +0x38: CurrentDirectory (CURDIR = UNICODE_STRING[16] + HANDLE[8] = 24 bytes)
    // +0x50: DllPath (UNICODE_STRING[16])
    // +0x60: ImagePathName (UNICODE_STRING[16])
    // +0x70: CommandLine (UNICODE_STRING[16])
    // +0x80: Environment (PVOID[8])
    let strings_base = 0x90usize;
    let mut string_off = strings_base;

    let cur_dir = b"C:\\";
    let cur_dir_utf16_len = (cur_dir.len() * 2) as u16;
    let cur_dir_addr = (proc_params_addr + string_off) as u64;
    for &b in cur_dir {
        params_buf[string_off] = b;
        params_buf[string_off + 1] = 0;
        string_off += 2;
    }
    string_off = (string_off + 7) & !7;
    write_u16(&mut params_buf, 0x38, cur_dir_utf16_len);
    write_u16(&mut params_buf, 0x3A, cur_dir_utf16_len);
    write_u32(&mut params_buf, 0x3C, 0);
    write_u64(&mut params_buf, 0x40, cur_dir_addr);
    write_u64(&mut params_buf, 0x48, 0); // CurrentDirectory.Handle

    // DllPath at +0x50
    let dll_path = b"C:\\Windows\\System32";
    let dll_path_utf16_len = (dll_path.len() * 2) as u16;
    let dll_path_addr = (proc_params_addr + string_off) as u64;
    for &b in dll_path {
        params_buf[string_off] = b;
        params_buf[string_off + 1] = 0;
        string_off += 2;
    }
    string_off = (string_off + 7) & !7;
    write_u16(&mut params_buf, 0x50, dll_path_utf16_len);
    write_u16(&mut params_buf, 0x52, dll_path_utf16_len);
    write_u32(&mut params_buf, 0x54, 0);
    write_u64(&mut params_buf, 0x58, dll_path_addr);

    // ImagePathName at +0x60
    let img_path = alloc::format!("C:\\test_exit.exe");
    let img_path_utf16_len = (img_path.len() * 2) as u16;
    let img_path_addr = (proc_params_addr + string_off) as u64;
    for &b in img_path.as_bytes() {
        params_buf[string_off] = b;
        params_buf[string_off + 1] = 0;
        string_off += 2;
    }
    string_off = (string_off + 7) & !7;
    write_u16(&mut params_buf, 0x60, img_path_utf16_len);
    write_u16(&mut params_buf, 0x62, img_path_utf16_len);
    write_u32(&mut params_buf, 0x64, 0);
    write_u64(&mut params_buf, 0x68, img_path_addr);

    // CommandLine at +0x70
    let cmd_line = alloc::format!("test_exit.exe");
    let cmd_line_utf16_len = (cmd_line.len() * 2) as u16;
    let cmd_line_addr = (proc_params_addr + string_off) as u64;
    for &b in cmd_line.as_bytes() {
        params_buf[string_off] = b;
        params_buf[string_off + 1] = 0;
        string_off += 2;
    }
    string_off = (string_off + 7) & !7;
    write_u16(&mut params_buf, 0x70, cmd_line_utf16_len);
    write_u16(&mut params_buf, 0x72, cmd_line_utf16_len);
    write_u32(&mut params_buf, 0x74, 0);
    write_u64(&mut params_buf, 0x78, cmd_line_addr);

    // Environment at +0x80
    let env_addr = (proc_params_addr + string_off) as u64;
    write_u16(&mut params_buf, string_off, 0);
    write_u16(&mut params_buf, string_off + 2, 0);
    write_u64(&mut params_buf, 0x80, env_addr);
    write_u64(&mut peb_buf, 0x10, image_base);
    write_u32(&mut peb_buf, layout::PEB_OFFSET_OS_MAJOR, 10);
    write_u32(&mut peb_buf, layout::PEB_OFFSET_OS_MINOR, 0);
    write_u16(&mut peb_buf, layout::PEB_OFFSET_OS_BUILD, 26100);
    write_u32(&mut peb_buf, layout::PEB_OFFSET_NUMBER_OF_PROCESSORS, 1);

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
        ntdll_init,
        exe_init,
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
        init_head,
        ntdll_init,
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
    let exe_name_buf_addr = ntdll_entry_addr + PAGE_SIZE;
    task.allocate_data_pages(exe_name_buf_addr, 1)?;

    write_u16(&mut exe_entry_buf, layout::LDR_ENTRY_OFFSET_LOAD_COUNT, 1);

    fn write_unicode_string_entry(
        buf: &mut [u8],
        offset: usize,
        length: u16,
        max_length: u16,
        buffer_addr: u64,
    ) {
        write_u16(buf, offset, length);
        write_u16(buf, offset + 2, max_length);
        write_u32(buf, offset + 4, 0);
        write_u64(buf, offset + 8, buffer_addr);
    }

    let exe_full_name = b"\\System32\\test_exit.exe";
    let exe_base_name = b"test_exit.exe";
    let ntdll_full_name = b"\\System32\\ntdll.dll";
    let ntdll_base_name = b"ntdll.dll";

    let exe_full_name_utf16_len = (exe_full_name.len() * 2) as u16;
    let exe_base_name_utf16_len = (exe_base_name.len() * 2) as u16;
    let ntdll_full_name_utf16_len = (ntdll_full_name.len() * 2) as u16;
    let ntdll_base_name_utf16_len = (ntdll_base_name.len() * 2) as u16;

    let mut name_buf = [0u8; PAGE_SIZE];
    let mut name_off = 0usize;

    let exe_full_name_addr = (exe_name_buf_addr + name_off) as u64;
    for &b in exe_full_name {
        name_buf[name_off] = b;
        name_buf[name_off + 1] = 0;
        name_off += 2;
    }
    name_off = (name_off + 7) & !7;

    let exe_base_name_addr = (exe_name_buf_addr + name_off) as u64;
    for &b in exe_base_name {
        name_buf[name_off] = b;
        name_buf[name_off + 1] = 0;
        name_off += 2;
    }
    name_off = (name_off + 7) & !7;

    let ntdll_full_name_addr = (exe_name_buf_addr + name_off) as u64;
    for &b in ntdll_full_name {
        name_buf[name_off] = b;
        name_buf[name_off + 1] = 0;
        name_off += 2;
    }
    name_off = (name_off + 7) & !7;

    let ntdll_base_name_addr = (exe_name_buf_addr + name_off) as u64;
    for &b in ntdll_base_name {
        name_buf[name_off] = b;
        name_buf[name_off + 1] = 0;
        name_off += 2;
    }

    write_unicode_string_entry(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_FULL_DLL_NAME,
        exe_full_name_utf16_len,
        exe_full_name_utf16_len,
        exe_full_name_addr,
    );
    write_unicode_string_entry(
        &mut exe_entry_buf,
        layout::LDR_ENTRY_OFFSET_BASE_DLL_NAME,
        exe_base_name_utf16_len,
        exe_base_name_utf16_len,
        exe_base_name_addr,
    );

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
        exe_init,
        init_head,
    );
    write_u64(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_DLL_BASE,
        ntdll.ntdll_entry_address,
    );
    write_u64(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_ENTRY_POINT,
        ntdll.ntdll_entry_point,
    );
    write_u32(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_SIZE_OF_IMAGE,
        0x0043_3000,
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

    write_unicode_string_entry(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_FULL_DLL_NAME,
        ntdll_full_name_utf16_len,
        ntdll_full_name_utf16_len,
        ntdll_full_name_addr,
    );
    write_unicode_string_entry(
        &mut ntdll_entry_buf,
        layout::LDR_ENTRY_OFFSET_BASE_DLL_NAME,
        ntdll_base_name_utf16_len,
        ntdll_base_name_utf16_len,
        ntdll_base_name_addr,
    );

    write_bytes(task, teb_addr, &teb_buf)?;
    write_bytes(task, peb_addr, &peb_buf)?;
    write_bytes(task, ldr_addr, &ldr_buf)?;
    write_bytes(task, exe_entry_addr, &exe_entry_buf)?;
    write_bytes(task, ntdll_entry_addr, &ntdll_entry_buf)?;
    write_bytes(task, proc_params_addr, &params_buf)?;
    write_bytes(task, exe_name_buf_addr, &name_buf)?;

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
            *((kva + 0x0260) as *mut u16) = 26100;
            // Cookie for RtlEncodeSystemPointer (0x7FFE0330)
            *((kva + 0x0330) as *mut u64) = 0xCAFE_BABE_DEAD_BEEFu64;
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

    let mut x = [0u64; 31];
    x[0] = user_entry;
    let context = NtdllContext {
        context_flags: NtdllContext::CONTEXT_FULL,
        cpsr: 0,
        sp: stack_pointer,
        pc: user_entry,
        x,
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
