mod syscall_table;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use spin::Mutex;

use crate::abi::AbiModule;
use crate::abi::windows::error::{
    STATUS_INFO_LENGTH_MISMATCH, STATUS_INVALID_HANDLE, STATUS_INVALID_PARAMETER,
    STATUS_NOT_IMPLEMENTED, STATUS_OBJECT_NAME_NOT_FOUND, STATUS_SUCCESS,
};
use crate::abi::windows::object::{
    NtEventObject, NtFileObject, NtObject, NtObjectTable, NtSectionObject, STD_ERROR_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use crate::abi::windows::peb;
use crate::arch::Trapframe;
use crate::environment::PAGE_SIZE;
use crate::fs::{SeekFrom, VfsManager, drivers::overlayfs::OverlayFS};
use crate::late_initcall;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::register_abi;
use crate::task::mytask;
use crate::task::namespace::TaskNamespace;
use crate::task::pe_loader::headers::{
    DOS_MAGIC, DosHeader, IMAGE_FILE_MACHINE_ARM64, PE_SIGNATURE, PE32PLUS_MAGIC,
};
use crate::task::pe_loader::{
    find_export_by_name, find_ordinal_only_export, load_pe_from_bytes, load_pe_into_task,
};
use crate::task::{AbiZone, Task};
use crate::vm;

pub const ABI_NAME: &str = "windows-aarch64";

#[derive(Clone, Default)]
struct WindowsProcessState {
    object_table: NtObjectTable,
    peb_address: u64,
    teb_address: u64,
    context_address: u64,
    ntdll_base: u64,
    ntdll_entry_point: u64,
    heap_base: usize,
    heap_current: usize,
    heap_end: usize,
}

const NTDLL_IMAGE_BASE: u64 = 0x0000_0001_8000_0000;
const WIN_TEB_BASE: usize = 0x0000_0002_0000_0000;
const WIN_PEB_BASE: usize = 0x0000_0002_0001_0000;
const WIN_LDR_BASE: usize = 0x0000_0002_0002_0000;
const WIN_LDR_ENTRIES_BASE: usize = 0x0000_0002_0003_0000;
const WIN_HEAP_BASE: usize = 0x0000_0002_0004_1000;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_WRITECOPY: u32 = 0x08;
const PAGE_EXECUTE: u32 = 0x10;
const PAGE_EXECUTE_READ: u32 = 0x20;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_FREE: u32 = 0x10000;
const MEM_PRIVATE: u32 = 0x20000;
const MEM_MAPPED: u32 = 0x40000;
const MEM_IMAGE: u32 = 0x1000000;

#[derive(Clone)]
pub struct WindowsAarch64Abi {
    namespace: Arc<TaskNamespace>,
    state: Arc<Mutex<WindowsProcessState>>,
}

impl Default for WindowsAarch64Abi {
    fn default() -> Self {
        Self {
            namespace: crate::task::namespace::get_root_namespace().clone(),
            state: Arc::new(Mutex::new(WindowsProcessState::default())),
        }
    }
}

impl AbiModule for WindowsAarch64Abi {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        ABI_NAME
    }

    fn get_name(&self) -> String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }

    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        let syscall_number = (trapframe.esr_el1 & 0xFFFF) as u16;
        let mut args = [0usize; 8];
        args.copy_from_slice(&trapframe.regs.reg[0..8]);

        let ret = self.dispatch_syscall(syscall_number, args, trapframe);

        trapframe.regs.reg[0] = ret;
        Ok(ret)
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        _file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        let file_obj = file_object.as_file()?;

        let mut magic_buffer = [0u8; 2];
        file_obj.seek(SeekFrom::Start(0)).ok();
        if file_obj.read(&mut magic_buffer).ok()? < 2 {
            return None;
        }
        if magic_buffer != DOS_MAGIC.to_le_bytes() {
            return None;
        }

        let mut lfanew_buffer = [0u8; 4];
        file_obj
            .seek(SeekFrom::Start(DosHeader::LFANEW_OFFSET as u64))
            .ok();
        if file_obj.read(&mut lfanew_buffer).ok()? < 4 {
            return None;
        }
        let pe_offset = u32::from_le_bytes(lfanew_buffer) as usize;

        let mut pe_sig_buffer = [0u8; 4];
        file_obj.seek(SeekFrom::Start(pe_offset as u64)).ok();
        if file_obj.read(&mut pe_sig_buffer).ok()? < 4 {
            return None;
        }
        if u32::from_le_bytes(pe_sig_buffer) != PE_SIGNATURE {
            return None;
        }

        let coff_offset = pe_offset + 4;
        let mut machine_buffer = [0u8; 2];
        file_obj.seek(SeekFrom::Start(coff_offset as u64)).ok();
        if file_obj.read(&mut machine_buffer).ok()? < 2 {
            return None;
        }
        if u16::from_le_bytes(machine_buffer) != IMAGE_FILE_MACHINE_ARM64 {
            return None;
        }

        let opt_offset = coff_offset + 20;
        let mut opt_magic_buffer = [0u8; 2];
        file_obj.seek(SeekFrom::Start(opt_offset as u64)).ok();
        if file_obj.read(&mut opt_magic_buffer).ok()? < 2 {
            return None;
        }
        if u16::from_le_bytes(opt_magic_buffer) != PE32PLUS_MAGIC {
            return None;
        }

        let mut confidence: u8 = 85;
        if let Some(abi) = current_abi
            && abi.get_name() == Self::name()
        {
            confidence = confidence.saturating_add(15);
        }
        Some(confidence.min(100))
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        _envp: &[&str],
        task: &crate::task::Task,
        trapframe: &mut Trapframe,
    ) -> Result<(), &'static str> {
        let file = file_object.as_file().ok_or("Invalid file object")?;

        for m in task.vm_manager.remove_all_memory_maps() {
            task.vm_manager
                .unmap_range_from_mmu(m.vmarea.start, m.vmarea.end);
        }

        let pe = load_pe_into_task(file, task, None).map_err(|_| "Failed to load PE image")?;
        let ntdll = self.load_ntdll(task)?;

        self.populate_ldr_system_dll_init_block(task, &ntdll)?;

        // Register ABI zones so user-space SVCs from ntdll/exe are routed to this ABI
        let ntdll_start = NTDLL_IMAGE_BASE as usize;
        let ntdll_end = (NTDLL_IMAGE_BASE + ntdll.image_size) as usize;
        unsafe {
            let abi_zones = task.abi_zones.get_mut();
            abi_zones.insert(
                ntdll_start,
                AbiZone {
                    range: ntdll_start..ntdll_end,
                    abi: Box::new(self.clone()),
                },
            );
            if pe.image_size > 0 {
                let exe_start = pe.image_base as usize;
                let exe_end = (pe.image_base + pe.image_size) as usize;
                abi_zones.insert(
                    exe_start,
                    AbiZone {
                        range: exe_start..exe_end,
                        abi: Box::new(self.clone()),
                    },
                );
            }
        }

        let mut state = self.state.lock();
        state.object_table = NtObjectTable::new();
        state.object_table.register_console_pseudo_handles();
        state.heap_base = 0;
        state.heap_current = 0;
        state.heap_end = 0;
        state.ntdll_base = ntdll.image_base;
        state.ntdll_entry_point = ntdll.entry_point;

        let (_stack_base, stack_top) = vm::setup_user_stack(task);
        let command_line = argv.join(" ");
        let command_line_bytes = command_line.as_bytes();
        let mut sp = stack_top;

        sp = sp.saturating_sub(command_line_bytes.len() + 1);
        copy_to_user(task, sp, command_line_bytes).map_err(|_| "Failed to copy command line")?;
        copy_to_user(task, sp + command_line_bytes.len(), &[0])
            .map_err(|_| "Failed to terminate command line")?;
        let command_line_ptr = sp as u64;

        sp = sp.saturating_sub(8);
        write_u64_user(task, sp, command_line_ptr)?;
        sp &= !0xF;

        let ldr_initialize_thunk = ntdll
            .ldr_initialize_thunk
            .unwrap_or(ntdll.entry_point)
            .max(ntdll.entry_point);

        let env = peb::initialize_process_environment(
            task,
            pe.image_base,
            pe.entry_point,
            pe.image_size,
            0,
            peb::NtDllData {
                ldr_data_address: 0,
                image_entry_address: 0,
                ntdll_entry_address: ntdll.image_base,
                ntdll_entry_point: ntdll.entry_point,
            },
            ldr_initialize_thunk,
            sp as u64,
            WIN_TEB_BASE,
            WIN_PEB_BASE,
            WIN_LDR_BASE,
            WIN_LDR_ENTRIES_BASE,
            WIN_LDR_ENTRIES_BASE + PAGE_SIZE,
        )?;
        state.peb_address = env.peb_address;
        state.teb_address = env.teb_address;
        state.context_address = env.context_address;
        drop(state);

        let context_ptr = env.context_address;
        trapframe.elr = ldr_initialize_thunk;
        trapframe.sp = context_ptr;
        trapframe.regs.reg[0] = context_ptr as usize;
        trapframe.regs.reg[1] = ntdll.image_base as usize;
        trapframe.tpidr_el0 = env.teb_address;
        trapframe.regs.reg[18] = env.teb_address as usize; // x18 = TEB on ARM64 Windows

        task.set_entry_point(ldr_initialize_thunk as usize);
        {
            let mut vcpu = task.vcpu.lock();
            vcpu.set_sp(context_ptr as usize);
            vcpu.set_tpidr_el0(env.teb_address);
            vcpu.set_pc(ldr_initialize_thunk);
        }

        // Verify exe PE headers are readable at image_base
        {
            let kva = task.vm_manager.translate_to_kva(pe.image_base as usize);
            if let Some(kva) = kva {
                let mz = unsafe { core::ptr::read_volatile(kva as *const u16) };
                let pe_off = unsafe { core::ptr::read_volatile((kva + 0x3C) as *const u32) };
                let pe_sig = if (pe_off as usize) + 4 <= PAGE_SIZE {
                    unsafe { core::ptr::read_volatile((kva + pe_off as usize) as *const u32) }
                } else {
                    0
                };
                crate::println!(
                    "[win-abi] exe headers at 0x{:x}: MZ=0x{:04X} e_lfanew=0x{:X} PE_sig=0x{:08X}",
                    pe.image_base,
                    mz,
                    pe_off,
                    pe_sig
                );
            } else {
                crate::println!(
                    "[win-abi] WARNING: exe image_base 0x{:x} NOT mapped!",
                    pe.image_base
                );
            }

            // Verify PEB fields
            if let Some(kva) = task.vm_manager.translate_to_kva(WIN_PEB_BASE) {
                let image_base = unsafe { core::ptr::read_volatile((kva + 0x10) as *const u64) };
                let ldr_ptr = unsafe { core::ptr::read_volatile((kva + 0x18) as *const u64) };
                let proc_params = unsafe { core::ptr::read_volatile((kva + 0x20) as *const u64) };
                let heap = unsafe { core::ptr::read_volatile((kva + 0x30) as *const u64) };
                crate::println!(
                    "[win-abi] PEB: ImageBase=0x{:x} Ldr=0x{:x} ProcParams=0x{:x} Heap=0x{:x}",
                    image_base,
                    ldr_ptr,
                    proc_params,
                    heap
                );
            }

            // Verify TEB fields
            if let Some(kva) = task.vm_manager.translate_to_kva(WIN_TEB_BASE) {
                let peb_ptr = unsafe { core::ptr::read_volatile((kva + 0x60) as *const u64) };
                let self_ptr = unsafe { core::ptr::read_volatile((kva + 0x30) as *const u64) };
                crate::println!("[win-abi] TEB: Self=0x{:x} PEB=0x{:x}", self_ptr, peb_ptr);
            }

            // Verify Ldr data
            if let Some(kva) = task.vm_manager.translate_to_kva(WIN_LDR_BASE) {
                let init_flag = unsafe { core::ptr::read_volatile((kva + 0x04) as *const u8) };
                let load_flink = unsafe { core::ptr::read_volatile((kva + 0x10) as *const u64) };
                let load_blink = unsafe { core::ptr::read_volatile((kva + 0x18) as *const u64) };
                crate::println!(
                    "[win-abi] Ldr: Initialized={} LoadFlink=0x{:x} LoadBlink=0x{:x}",
                    init_flag,
                    load_flink,
                    load_blink
                );
            }
        }

        crate::println!(
            "[win-abi] trapframe set: elr=0x{:x} sp=0x{:x} x0=0x{:x} tpidr_el0=0x{:x}",
            trapframe.elr,
            trapframe.sp,
            trapframe.regs.reg[0],
            trapframe.tpidr_el0
        );
        Ok(())
    }

    fn get_task_namespace(&self) -> Arc<TaskNamespace> {
        self.namespace.clone()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn setup_overlay_environment(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
        system_path: &str,
        config_path: &str,
    ) -> Result<(), &'static str> {
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
                    "Failed to create overlay filesystem for Windows ABI: {}",
                    e.message
                );
                return Err("Failed to create Windows overlay environment");
            }
        };
        match target_vfs.mount(fs, "/", 0) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!("Failed to mount overlay for Windows ABI: {}", e.message);
                Err("Failed to create Windows overlay environment")
            }
        }
    }
}

impl WindowsAarch64Abi {
    fn nt_allocate_virtual_memory(&mut self, base_ptr: usize, size_ptr: usize) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let requested_base = read_u64_user(task, base_ptr).unwrap_or(0) as usize;
        let requested_size = read_u64_user(task, size_ptr).unwrap_or(0) as usize;

        let size = align_up(
            if requested_size == 0 {
                PAGE_SIZE
            } else {
                requested_size
            },
            PAGE_SIZE,
        );
        let base = if requested_base == 0 {
            match task.vm_manager.find_unmapped_area(size, PAGE_SIZE) {
                Some(addr) => addr,
                None => return status(STATUS_INVALID_PARAMETER),
            }
        } else {
            align_down(requested_base, PAGE_SIZE)
        };

        // Check if the region is already mapped — MEM_COMMIT on existing reservation is a NOP
        let already_mapped = (0..size)
            .step_by(PAGE_SIZE)
            .any(|off| task.vm_manager.translate_to_kva(base + off).is_some());

        if !already_mapped {
            let pages = size / PAGE_SIZE;
            if task.allocate_data_pages(base, pages).is_err() {
                return status(STATUS_INVALID_PARAMETER);
            }
        }

        if write_u64_user(task, base_ptr, base as u64).is_err()
            || write_u64_user(task, size_ptr, size as u64).is_err()
        {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_free_virtual_memory(&mut self, base_ptr: usize, size_ptr: usize) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let base = align_down(
            read_u64_user(task, base_ptr).unwrap_or(0) as usize,
            PAGE_SIZE,
        );
        let size = align_up(
            read_u64_user(task, size_ptr).unwrap_or(PAGE_SIZE as u64) as usize,
            PAGE_SIZE,
        );
        if base == 0 || size == 0 {
            return status(STATUS_INVALID_PARAMETER);
        }

        task.free_data_pages(base, size / PAGE_SIZE);
        let _ = write_u64_user(task, size_ptr, 0);

        status(STATUS_SUCCESS)
    }

    fn nt_close(&mut self, handle: u32) -> usize {
        if handle == STD_INPUT_HANDLE || handle == STD_OUTPUT_HANDLE || handle == STD_ERROR_HANDLE {
            return status(STATUS_SUCCESS);
        }
        let mut state = self.state.lock();
        if state.object_table.remove(handle).is_some() {
            status(STATUS_SUCCESS)
        } else {
            status(STATUS_INVALID_HANDLE)
        }
    }

    fn nt_create_file(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let out_handle_ptr = args[0];
        let obj_attr_ptr = args[2];

        let obj_attr = match read_object_attributes_from_user(task, obj_attr_ptr) {
            Ok(attr) => attr,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };

        let nt_path = translate_nt_path(&obj_attr.object_name);

        let vfs = match get_vfs_for_task(task) {
            Some(vfs) => vfs,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let object = match vfs.open(&nt_path, 0) {
            Ok(obj) => obj,
            Err(_) => return status(STATUS_OBJECT_NAME_NOT_FOUND),
        };

        let file = match object {
            crate::object::KernelObject::File(file) => file,
            _ => return status(STATUS_INVALID_PARAMETER),
        };

        let mut state = self.state.lock();
        let handle = state.object_table.insert(NtObject::File(NtFileObject {
            file,
            path: Some(nt_path),
        }));
        drop(state);

        if write_u32_user(task, out_handle_ptr, handle).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_read_file(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let handle = args[0] as u32;
        let buffer_ptr = args[5];
        let length = args[6];
        if length == 0 || length > 0x100000 {
            return status(STATUS_INVALID_PARAMETER);
        }

        let file = {
            let state = self.state.lock();
            match state.object_table.get(handle) {
                Some(NtObject::File(file)) => file.file.clone(),
                _ => return status(STATUS_INVALID_HANDLE),
            }
        };

        let mut tmp = Vec::new();
        tmp.resize(length, 0);
        let read = match file.read(&mut tmp) {
            Ok(read) => read,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };

        if copy_to_user(task, buffer_ptr, &tmp[..read]).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_write_file(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let handle = args[0] as u32;
        let buffer_ptr = args[5];
        let length = args[6];
        if length == 0 || length > 0x100000 {
            return status(STATUS_INVALID_PARAMETER);
        }

        let mut buffer = Vec::new();
        buffer.resize(length, 0);
        if copy_from_user(task, buffer_ptr, &mut buffer).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        if handle == STD_OUTPUT_HANDLE || handle == STD_ERROR_HANDLE {
            let text = String::from_utf8_lossy(&buffer);
            crate::println!("{}", text);
            return status(STATUS_SUCCESS);
        }

        if handle == STD_INPUT_HANDLE {
            return status(STATUS_INVALID_HANDLE);
        }

        let file = {
            let state = self.state.lock();
            match state.object_table.get(handle) {
                Some(NtObject::File(file)) => file.file.clone(),
                _ => return status(STATUS_INVALID_HANDLE),
            }
        };

        if file.write(&buffer).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_terminate_process(&mut self, exit_status: usize) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };
        task.exit(exit_status as i32);
        status(STATUS_SUCCESS)
    }

    fn nt_create_section(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let out_handle_ptr = args[0];
        let maximum_size_ptr = args[3];
        let section_page_protection = args[4] as u32;
        let allocation_attributes = args[5] as u32;
        let file_handle = args[6] as u32;

        let maximum_size = if maximum_size_ptr != 0 {
            match read_u64_user(task, maximum_size_ptr) {
                Ok(v) => v,
                Err(_) => return status(STATUS_INVALID_PARAMETER),
            }
        } else {
            0
        };

        let file = if file_handle != 0 {
            let state = self.state.lock();
            match state.object_table.get(file_handle) {
                Some(NtObject::File(file_obj)) => Some(file_obj.file.clone()),
                _ => return status(STATUS_INVALID_HANDLE),
            }
        } else {
            None
        };

        let section = NtSectionObject {
            file,
            maximum_size,
            section_page_protection,
            allocation_attributes,
        };

        if !is_supported_page_protection(section.section_page_protection) {
            return status(STATUS_INVALID_PARAMETER);
        }

        let mut state = self.state.lock();
        let handle = state.object_table.insert(NtObject::Section(section));
        drop(state);

        if write_u32_user(task, out_handle_ptr, handle).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_map_view_of_section(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let section_handle = args[0] as u32;
        let base_address_ptr = args[2];
        let view_size_ptr = args[6];

        if base_address_ptr == 0 || view_size_ptr == 0 {
            return status(STATUS_INVALID_PARAMETER);
        }

        let requested_base = match read_u64_user(task, base_address_ptr) {
            Ok(v) => v as usize,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };
        let requested_view_size = match read_u64_user(task, view_size_ptr) {
            Ok(v) => v as usize,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };

        let section = {
            let state = self.state.lock();
            match state.object_table.get(section_handle) {
                Some(NtObject::Section(section)) => section.clone(),
                _ => return status(STATUS_INVALID_HANDLE),
            }
        };

        let preferred_base = if requested_base == 0 {
            None
        } else {
            Some(align_down(requested_base, PAGE_SIZE) as u64)
        };

        let (actual_base, actual_size) = if let Some(file) = section.file.clone() {
            match load_pe_into_task(file.as_ref(), task, preferred_base) {
                Ok(load) => (load.image_base as usize, load.image_size as usize),
                Err(_) => return status(STATUS_INVALID_PARAMETER),
            }
        } else {
            let size = if requested_view_size != 0 {
                align_up(requested_view_size, PAGE_SIZE)
            } else if section.maximum_size != 0 {
                align_up(section.maximum_size as usize, PAGE_SIZE)
            } else {
                PAGE_SIZE
            };

            let base = if requested_base == 0 {
                match task.vm_manager.find_unmapped_area(size, PAGE_SIZE) {
                    Some(addr) => addr,
                    None => return status(STATUS_INVALID_PARAMETER),
                }
            } else {
                align_down(requested_base, PAGE_SIZE)
            };

            if task.allocate_data_pages(base, size / PAGE_SIZE).is_err() {
                return status(STATUS_INVALID_PARAMETER);
            }

            (base, size)
        };

        if write_u64_user(task, base_address_ptr, actual_base as u64).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }
        if write_u64_user(task, view_size_ptr, actual_size as u64).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_unmap_view_of_section(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let base_address = align_down(args[1], PAGE_SIZE);
        if base_address == 0 {
            return status(STATUS_INVALID_PARAMETER);
        }

        let map = match task.vm_manager.search_memory_map(base_address) {
            Some(map) => map,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let size = map
            .vmarea
            .end
            .saturating_sub(map.vmarea.start)
            .saturating_add(1);
        let pages = align_up(size, PAGE_SIZE) / PAGE_SIZE;
        task.free_data_pages(map.vmarea.start, pages);

        status(STATUS_SUCCESS)
    }

    fn nt_protect_virtual_memory(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let base_address_ptr = args[1];
        let region_size_ptr = args[2];
        let old_protect_ptr = args[4];
        let new_protect = args[3] as u32;

        if !is_supported_page_protection(new_protect) {
            return status(STATUS_INVALID_PARAMETER);
        }

        let base_address = match read_u64_user(task, base_address_ptr) {
            Ok(v) => v,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };
        let region_size = match read_u64_user(task, region_size_ptr) {
            Ok(v) => v,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };

        if write_u64_user(task, base_address_ptr, base_address).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }
        if write_u64_user(task, region_size_ptr, region_size).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }
        if old_protect_ptr != 0 && write_u32_user(task, old_protect_ptr, PAGE_READWRITE).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_query_virtual_memory(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let base_address = args[1];
        let info_class = args[2] as u32;
        let output_buffer = args[3];
        let output_len = args[4];
        let return_length_ptr = args[5];

        if info_class != 0 {
            if info_class == 6 {
                const MEMORY_REGION_INFORMATION_SIZE: usize = 24;
                if output_len < MEMORY_REGION_INFORMATION_SIZE {
                    return status(STATUS_INFO_LENGTH_MISMATCH);
                }
                let mut out = [0u8; MEMORY_REGION_INFORMATION_SIZE];
                let aligned_base = align_down(base_address, PAGE_SIZE) as u64;
                out[0..8].copy_from_slice(&aligned_base.to_le_bytes());
                out[8..16].copy_from_slice(&PAGE_SIZE.to_le_bytes());
                out[16..20].copy_from_slice(&0u32.to_le_bytes());
                out[20..24].copy_from_slice(&(MEM_COMMIT | PAGE_READWRITE).to_le_bytes());
                if copy_to_user(task, output_buffer, &out).is_err() {
                    return status(STATUS_INVALID_PARAMETER);
                }
                if return_length_ptr != 0 {
                    let _ = write_u64_user(
                        task,
                        return_length_ptr,
                        MEMORY_REGION_INFORMATION_SIZE as u64,
                    );
                }
                return status(STATUS_SUCCESS);
            }
            return status(STATUS_NOT_IMPLEMENTED);
        }

        const MEMORY_BASIC_INFORMATION_SIZE: usize = 48;

        let map = match task.vm_manager.search_memory_map(base_address) {
            Some(map) => map,
            None => {
                let mut out = [0u8; MEMORY_BASIC_INFORMATION_SIZE];
                out[0..8]
                    .copy_from_slice(&(align_down(base_address, PAGE_SIZE) as u64).to_le_bytes());
                out[8..16]
                    .copy_from_slice(&(align_down(base_address, PAGE_SIZE) as u64).to_le_bytes());
                out[24..32].copy_from_slice(&PAGE_SIZE.to_le_bytes());
                out[32..36].copy_from_slice(&MEM_FREE.to_le_bytes());
                let _ = copy_to_user(task, output_buffer, &out);
                if return_length_ptr != 0 {
                    let _ = write_u64_user(
                        task,
                        return_length_ptr,
                        MEMORY_BASIC_INFORMATION_SIZE as u64,
                    );
                }
                return status(STATUS_SUCCESS);
            }
        };

        if output_len < MEMORY_BASIC_INFORMATION_SIZE {
            return status(STATUS_INVALID_PARAMETER);
        }

        let region_size = map
            .vmarea
            .end
            .saturating_sub(map.vmarea.start)
            .saturating_add(1) as u64;
        let state = if map.permissions == 0 {
            MEM_RESERVE
        } else {
            MEM_COMMIT
        };
        let mem_type = section_type_to_memory_type(map.is_shared, map.owner.is_some());

        let mut out = [0u8; MEMORY_BASIC_INFORMATION_SIZE];
        out[0..8].copy_from_slice(&(map.vmarea.start as u64).to_le_bytes());
        out[8..16].copy_from_slice(&(map.vmarea.start as u64).to_le_bytes());
        out[16..20].copy_from_slice(&PAGE_READWRITE.to_le_bytes());
        out[24..32].copy_from_slice(&region_size.to_le_bytes());
        out[32..36].copy_from_slice(&state.to_le_bytes());
        out[36..40].copy_from_slice(&PAGE_READWRITE.to_le_bytes());
        out[40..44].copy_from_slice(&mem_type.to_le_bytes());

        if copy_to_user(task, output_buffer, &out).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        if return_length_ptr != 0
            && write_u64_user(
                task,
                return_length_ptr,
                MEMORY_BASIC_INFORMATION_SIZE as u64,
            )
            .is_err()
        {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_open_section(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let out_handle_ptr = args[0];
        let obj_attr_ptr = args[2];

        let obj_attr = match read_object_attributes_from_user(task, obj_attr_ptr) {
            Ok(attr) => attr,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };

        let nt_path = if obj_attr.root_directory != 0 {
            alloc::format!("/System32/{}", obj_attr.object_name)
        } else {
            translate_nt_path(&obj_attr.object_name)
        };

        let file = {
            let vfs = match get_vfs_for_task(task) {
                Some(vfs) => vfs,
                None => return status(STATUS_INVALID_PARAMETER),
            };

            match vfs.open(&nt_path, 0) {
                Ok(obj) => match obj {
                    crate::object::KernelObject::File(file) => Some(file),
                    _ => None,
                },
                Err(_) => None,
            }
        };

        let section = NtSectionObject {
            file,
            maximum_size: 0,
            section_page_protection: PAGE_READWRITE,
            allocation_attributes: 0x8000000,
        };

        let mut state = self.state.lock();
        let handle = state.object_table.insert(NtObject::Section(section));
        drop(state);

        if write_u32_user(task, out_handle_ptr, handle).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_query_section(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let section_handle = args[0] as u32;
        let info_class = args[1] as u32;
        let output_buffer = args[2];
        let output_buffer_size = args[3];
        let return_length_ptr = args[4];

        if info_class != 0 {
            return status(STATUS_NOT_IMPLEMENTED);
        }

        let section = {
            let state = self.state.lock();
            match state.object_table.get(section_handle) {
                Some(NtObject::Section(section)) => section.clone(),
                _ => return status(STATUS_INVALID_HANDLE),
            }
        };

        const SECTION_BASIC_INFORMATION_SIZE: usize = 24;
        if output_buffer_size < SECTION_BASIC_INFORMATION_SIZE {
            return status(STATUS_INVALID_PARAMETER);
        }

        let mut out = [0u8; SECTION_BASIC_INFORMATION_SIZE];
        out[0..8].copy_from_slice(&0u64.to_le_bytes());
        out[8..12].copy_from_slice(&section.allocation_attributes.to_le_bytes());
        out[16..24].copy_from_slice(&section.maximum_size.to_le_bytes());

        if copy_to_user(task, output_buffer, &out).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        if return_length_ptr != 0
            && write_u32_user(
                task,
                return_length_ptr,
                SECTION_BASIC_INFORMATION_SIZE as u32,
            )
            .is_err()
        {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_write_virtual_memory(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let base_address = args[1];
        let source_buffer = args[2];
        let number_of_bytes_to_write = args[3];
        let number_of_bytes_written_ptr = args[4];

        if number_of_bytes_to_write == 0 {
            if number_of_bytes_written_ptr != 0
                && write_u64_user(task, number_of_bytes_written_ptr, 0).is_err()
            {
                return status(STATUS_INVALID_PARAMETER);
            }
            return status(STATUS_SUCCESS);
        }

        let mut buffer = Vec::new();
        buffer.resize(number_of_bytes_to_write, 0);
        if copy_from_user(task, source_buffer, &mut buffer).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        if copy_to_user(task, base_address, &buffer).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        if number_of_bytes_written_ptr != 0
            && write_u64_user(
                task,
                number_of_bytes_written_ptr,
                number_of_bytes_to_write as u64,
            )
            .is_err()
        {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_get_current_process_id(&self) -> usize {
        match mytask() {
            Some(task) => task.get_id(),
            None => 0,
        }
    }

    fn rtl_allocate_heap(&mut self, args: [usize; 8]) -> usize {
        let size = align_up(args[2], 16);
        if size == 0 {
            return 0;
        }

        let mut state = self.state.lock();
        let current = state.heap_current;
        let next = current.saturating_add(size);
        if current == 0 || next > state.heap_end {
            return 0;
        }

        state.heap_current = next;
        current
    }

    fn nt_continue(&mut self, trapframe: &mut Trapframe, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let context_ptr = args[0];
        if context_ptr == 0 {
            return status(STATUS_INVALID_PARAMETER);
        }

        let context = match read_ntdll_context(task, context_ptr) {
            Ok(ctx) => ctx,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };

        for i in 0..31 {
            trapframe.regs.reg[i] = context.x[i] as usize;
        }
        trapframe.sp = context.sp;
        trapframe.elr = context.pc;
        trapframe.spsr = context.cpsr as u64;

        {
            let mut vcpu = task.vcpu.lock();
            vcpu.store(trapframe);
        }

        context.x[0] as usize
    }

    /// Populate the `PS_SYSTEM_DLL_INIT_BLOCK` (LdrSystemDllInitBlock) in ntdll's
    /// mapped memory before calling LdrInitializeThunk. The Windows kernel writes
    /// initialization data here so ntdll can configure CFG, mitigations, and SCP.
    fn populate_ldr_system_dll_init_block(
        &self,
        task: &crate::task::Task,
        ntdll: &NtDllLoadResult,
    ) -> Result<(), &'static str> {
        let rva = match ntdll.ldr_system_dll_init_block_rva {
            Some(rva) => rva,
            None => {
                crate::println!(
                    "[win-abi] Skipping LdrSystemDllInitBlock population: export not found"
                );
                return Ok(());
            }
        };

        let init_block_va = ntdll.image_base + rva as u64;

        // PS_SYSTEM_DLL_INIT_BLOCK_V3 for Windows 11 24H2 (build 26100)
        // Total size: 0x128 (296 bytes)
        const INIT_BLOCK_SIZE: usize = 0x128;
        let mut block = [0u8; INIT_BLOCK_SIZE];

        // Offset 0x00: Size = 0x128
        block[0..4].copy_from_slice(&(INIT_BLOCK_SIZE as u32).to_le_bytes());

        // Offset 0x10: SystemDllNativeRelocation = ntdll base address
        block[0x10..0x18].copy_from_slice(&ntdll.image_base.to_le_bytes());

        // Offset 0x98: RngData — non-zero random seed
        block[0x98..0x9C].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());

        // Offset 0xA0: MitigationOptionsMap (3 × ULONG64 = 24 bytes)
        // Set CFG enabled (bit 1 of first QWord) and standard mitigations
        let mitigation_flags: u64 = (1 << 1) | (1 << 2) | (1 << 3) | (1 << 12);
        block[0xA0..0xA8].copy_from_slice(&mitigation_flags.to_le_bytes());
        block[0xA8..0xB0].copy_from_slice(&0u64.to_le_bytes());
        block[0xB0..0xB8].copy_from_slice(&0u64.to_le_bytes());

        // Offset 0xB8: CfgBitMap = NULL (no CFG bitmap; ntdll must handle gracefully)
        // Offset 0xC0: CfgBitMapSize = 0
        // Offset 0xC8: Wow64CfgBitMap = NULL
        // Offset 0xD0: Wow64CfgBitMapSize = 0

        // Offset 0xD8: MitigationAuditOptionsMap (3 × ULONG64 = 24 bytes) — zeroed

        // Offsets 0xF0-0x127: SCP CFG function pointers (24H2) — zeroed/NULL
        // ScpCfgCheckFunction, ScpCfgCheckESFunction, ScpCfgDispatchFunction,
        // ScpCfgDispatchESFunction, ScpArm64EcCallCheck, ScpArm64EcCfgCheckFunction,
        // ScpArm64EcCfgCheckESFunction

        crate::println!(
            "[win-abi] Writing PS_SYSTEM_DLL_INIT_BLOCK (0x{:x} bytes) at VA 0x{:x}",
            INIT_BLOCK_SIZE,
            init_block_va
        );

        copy_to_user(task, init_block_va as usize, &block)
            .map_err(|_| "Failed to write LdrSystemDllInitBlock to ntdll memory")?;

        // Verify the write
        if let Some(kva) = task.vm_manager.translate_to_kva(init_block_va as usize) {
            let size_field = unsafe { core::ptr::read_volatile(kva as *const u32) };
            let native_reloc = unsafe { core::ptr::read_volatile((kva + 0x10) as *const u64) };
            crate::println!(
                "[win-abi] LdrSystemDllInitBlock verified: Size=0x{:x} NativeReloc=0x{:x}",
                size_field,
                native_reloc
            );
        }

        Ok(())
    }

    fn load_ntdll(&self, task: &crate::task::Task) -> Result<NtDllLoadResult, &'static str> {
        let vfs = get_vfs_for_task(task).ok_or("No VFS available")?;
        let obj = vfs
            .open("/System32/ntdll.dll", 0)
            .map_err(|_| "Failed to open /System32/ntdll.dll")?;
        let file = match obj {
            crate::object::KernelObject::File(f) => f,
            _ => return Err("ntdll is not a file"),
        };

        let mut ntdll_bytes = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => ntdll_bytes.extend_from_slice(&buf[..n]),
                Err(_) => return Err("Failed to read ntdll.dll"),
            }
        }

        let ntdll_slice = ntdll_bytes.leak();
        let load = load_pe_from_bytes(ntdll_slice, task, Some(NTDLL_IMAGE_BASE))
            .map_err(|_| "Failed to load ntdll PE image")?;

        let ldr_init_thunk_rva = find_export_by_name(ntdll_slice, "LdrInitializeThunk")
            .or_else(|| find_ordinal_only_export(ntdll_slice));
        let ldr_initialize_thunk = ldr_init_thunk_rva.map(|rva| load.image_base + rva as u64);
        let ldr_system_dll_init_block_rva =
            find_export_by_name(ntdll_slice, "LdrSystemDllInitBlock");

        if let Some(rva) = ldr_system_dll_init_block_rva {
            crate::println!(
                "[win-abi] LdrSystemDllInitBlock found at RVA 0x{:x} (VA 0x{:x})",
                rva,
                load.image_base + rva as u64
            );
        } else {
            crate::println!("[win-abi] WARNING: LdrSystemDllInitBlock export NOT found in ntdll");
        }

        Ok(NtDllLoadResult {
            image_base: load.image_base,
            image_size: load.image_size,
            entry_point: load.entry_point,
            ldr_initialize_thunk,
            ldr_system_dll_init_block_rva,
        })
    }
    /// Dispatch a syscall using the auto-generated syscall table.
    ///
    /// Looks up the syscall name by number from `syscall_table::lookup_by_number`,
    /// then dispatches to the correct handler. Unknown or unimplemented syscalls
    /// return `STATUS_NOT_IMPLEMENTED` with a trace log.
    fn dispatch_syscall(
        &mut self,
        number: u16,
        args: [usize; 8],
        trapframe: &mut Trapframe,
    ) -> usize {
        let name = match syscall_table::lookup_by_number(number) {
            Some(n) => n,
            None => {
                crate::println!("[win-abi] unknown syscall 0x{:04X} ({})", number, number);
                return status(STATUS_NOT_IMPLEMENTED);
            }
        };

        crate::println!(
            "[win-abi] syscall 0x{:04X}: {} (pc=0x{:x})",
            number,
            name,
            trapframe.elr
        );

        match name {
            "NtAllocateVirtualMemory" => self.nt_allocate_virtual_memory(args[1], args[3]),
            "NtAllocateVirtualMemoryEx" => self.nt_allocate_virtual_memory(args[1], args[2]),

            "NtFreeVirtualMemory" => self.nt_free_virtual_memory(args[1], args[2]),
            "NtClose" => self.nt_close(args[0] as u32),
            "NtReadFile" => self.nt_read_file(args),
            "NtCreateFile" => self.nt_create_file(args),
            "NtWriteFile" => self.nt_write_file(args),
            "NtQueryVirtualMemory" => self.nt_query_virtual_memory(args),
            "NtMapViewOfSection" => self.nt_map_view_of_section(args),
            "NtUnmapViewOfSection" => self.nt_unmap_view_of_section(args),
            "NtTerminateProcess" => self.nt_terminate_process(args[1]),
            "NtOpenSection" => self.nt_open_section(args),
            "NtWriteVirtualMemory" => self.nt_write_virtual_memory(args),
            "NtCreateSection" => self.nt_create_section(args),
            "NtProtectVirtualMemory" => self.nt_protect_virtual_memory(args),
            "NtQuerySection" => self.nt_query_section(args),
            "NtContinue" => self.nt_continue(trapframe, args),

            "NtCreateEvent" => {
                let handle = self
                    .state
                    .lock()
                    .object_table
                    .insert(NtObject::Event(NtEventObject));
                let task = mytask().unwrap();
                let _ = write_u64_user(task, args[1], handle as u64);
                status(STATUS_SUCCESS)
            }
            "NtOpenKey" => {
                let handle = self.state.lock().object_table.insert(NtObject::Null);
                let task = mytask().unwrap();
                let _ = write_u64_user(task, args[0], handle as u64);
                status(STATUS_SUCCESS)
            }
            "NtQueryValueKey" => status(STATUS_OBJECT_NAME_NOT_FOUND),

            "NtQueryInformationProcess" => {
                let task = mytask().unwrap();
                let info_class = args[1] as u32;
                let buf = args[2];
                let len = args[3];
                if len >= 16 && buf != 0 {
                    let mut out = [0u8; 16];
                    out[0..8].copy_from_slice(&(task.get_id() as u64).to_le_bytes());
                    let _ = copy_to_user(task, buf, &out);
                }
                if args[4] != 0 {
                    let _ = write_u64_user(task, args[4], 16);
                }
                status(STATUS_SUCCESS)
            }
            "NtQueryPerformanceCounter" => {
                let task = match mytask() {
                    Some(t) => t,
                    None => return status(STATUS_INVALID_PARAMETER),
                };
                let buf = args[0];
                let _len = args[1];
                if buf != 0 {
                    let counter = (crate::timer::get_tick() as u64) * 10000;
                    let _ = write_u64_user(task, buf, counter);
                }
                status(STATUS_SUCCESS)
            }

            "NtQuerySystemInformation" => {
                let task = match mytask() {
                    Some(t) => t,
                    None => return status(STATUS_INVALID_PARAMETER),
                };
                let info_class = args[0] as u32;
                let buf = args[1];
                let len = args[2];
                crate::println!("[win-abi] NtQuerySystemInformation class={}", info_class);
                match info_class {
                    0x00 => {
                        // SystemBasicInformation (48 bytes)
                        if len >= 48 && buf != 0 {
                            let mut out = [0u8; 48];
                            out[0..4].copy_from_slice(&0u32.to_le_bytes()); // OemId
                            out[4..8].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes()); // PageSize
                            out[8..16].copy_from_slice(&0x10000u64.to_le_bytes()); // MinAddr
                            out[16..24].copy_from_slice(&0x7FFFFFFEFFFFu64.to_le_bytes()); // MaxAddr
                            out[24..32].copy_from_slice(&1u64.to_le_bytes()); // ActiveProcessorMask
                            out[32..36].copy_from_slice(&1u32.to_le_bytes()); // NumberOfProcessors
                            out[36..40].copy_from_slice(&0x0FCCu32.to_le_bytes()); // ProcessorType (ARM64)
                            out[40..44].copy_from_slice(&(64 * 1024u32).to_le_bytes()); // AllocationGranularity
                            out[44..46].copy_from_slice(&1u16.to_le_bytes()); // ProcessorLevel
                            out[46..48].copy_from_slice(&0u16.to_le_bytes()); // ProcessorRevision
                            let _ = copy_to_user(task, buf, &out);
                        }
                    }
                    0x03 => {
                        // SystemTimeOfDayInformation (32 bytes)
                        if len >= 16 && buf != 0 {
                            let mut out = [0u8; 32];
                            let tick = crate::timer::get_tick() as u64;
                            let ft = tick * 10000 + 132477120000000000;
                            out[0..8].copy_from_slice(&ft.to_le_bytes()); // KeBootTime
                            out[8..16].copy_from_slice(&ft.to_le_bytes()); // KeCurrentTime
                            let _ = copy_to_user(task, buf, &out);
                        }
                    }
                    0x05 => {
                        // SystemProcessInformation
                        // Return STATUS_INFO_LENGTH_MISMATCH to indicate more data needed
                    }
                    0x3E => {
                        // SystemCodeIntegrityInformation (class 62)
                        // Return minimal 4-byte struct: ULONG flags = 0
                        if len >= 4 && buf != 0 {
                            let out = 0u32.to_le_bytes();
                            let _ = copy_to_user(task, buf, &out);
                        }
                    }
                    _ => {}
                }
                status(STATUS_SUCCESS)
            }

            "NtQuerySystemTime" => {
                let task = match mytask() {
                    Some(t) => t,
                    None => return status(STATUS_INVALID_PARAMETER),
                };
                let buf = args[0];
                if buf != 0 {
                    let tick = crate::timer::get_tick() as u64;
                    let ft = tick * 10000 + 132477120000000000;
                    let _ = write_u64_user(task, buf, ft);
                }
                status(STATUS_SUCCESS)
            }

            "NtQuerySystemInformationEx" => {
                let task = match mytask() {
                    Some(t) => t,
                    None => return status(STATUS_INVALID_PARAMETER),
                };
                let info_class = args[0] as u32;
                let output_buffer = args[3];
                let output_len = args[4];
                crate::println!("[win-abi] NtQuerySystemInformationEx class={}", info_class);
                match info_class {
                    0x07 => {
                        // SystemProcessorFeaturesInformation (56 bytes)
                        if output_len >= 56 && output_buffer != 0 {
                            let mut out = [0u8; 56];
                            out[0..4].copy_from_slice(&1u32.to_le_bytes()); // ProcessorFeature
                            let _ = copy_to_user(task, output_buffer, &out);
                        }
                    }
                    _ => {}
                }
                status(STATUS_SUCCESS)
            }

            "NtWorkerFactoryWorkerReady"
            | "NtManageHotPatch"
            | "NtAcceptConnectPort"
            | "NtRemoveIoCompletion"
            | "NtQueryObject"
            | "NtQueryInformationFile"
            | "NtReleaseMutant"
            | "NtOpenProcess"
            | "NtAccessCheckAndAuditAlarm"
            | "NtQueryDirectoryFile"
            | "NtQueryAttributesFile"
            | "NtClearEvent"
            | "NtReadVirtualMemory"
            | "NtOpenEvent"
            | "NtQueryEvent"
            | "NtDelayExecution"
            | "NtWaitForMultipleObjects"
            | "NtTraceEvent"
            | "NtRaiseHardError" => status(STATUS_SUCCESS),

            _ => {
                crate::println!("[win-abi] unimplemented syscall 0x{:04X}: {}", number, name);
                status(STATUS_NOT_IMPLEMENTED)
            }
        }
    }
}

struct NtDllLoadResult {
    image_base: u64,
    image_size: u64,
    entry_point: u64,
    ldr_initialize_thunk: Option<u64>,
    /// RVA of the LdrSystemDllInitBlock export in ntdll
    ldr_system_dll_init_block_rva: Option<u32>,
}

fn status(code: u32) -> usize {
    code as usize
}

fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn is_supported_page_protection(protect: u32) -> bool {
    matches!(
        protect,
        0 | PAGE_NOACCESS
            | PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
    )
}

fn section_type_to_memory_type(is_shared: bool, has_owner: bool) -> u32 {
    if has_owner {
        MEM_IMAGE
    } else if is_shared {
        MEM_MAPPED
    } else {
        MEM_PRIVATE
    }
}

fn get_vfs_for_task(task: &crate::task::Task) -> Option<Arc<crate::fs::VfsManager>> {
    task.get_vfs()
        .or_else(crate::fs::vfs_v2::manager::get_global_vfs_manager_safe)
}

fn translate_nt_path(nt_path: &str) -> String {
    let path = nt_path.replace('\\', "/");

    let path = if path.starts_with("//?/") || path.starts_with("\\??\\") {
        &path[4..]
    } else if path.starts_with("//./") {
        &path[4..]
    } else {
        &path
    };

    let path = path.trim_start_matches('/');

    if path.starts_with("SystemRoot/") || path.starts_with("systemroot/") {
        alloc::format!("/{}", &path[11..])
    } else if path.len() > 2 && path.as_bytes()[1] == b':' {
        let rest = &path[2..];
        let rest = rest.trim_start_matches('/');
        if rest.starts_with("Windows/") || rest.starts_with("windows/") {
            alloc::format!("/{}", &rest[8..])
        } else {
            alloc::format!("/{}", rest)
        }
    } else if path.starts_with("Windows/") || path.starts_with("windows/") {
        alloc::format!("/{}", &path[8..])
    } else {
        alloc::format!("/System32/{}", path)
    }
}

fn read_u64_user(task: &crate::task::Task, user_addr: usize) -> Result<u64, &'static str> {
    let mut bytes = [0u8; 8];
    copy_from_user(task, user_addr, &mut bytes).map_err(|_| "copy_from_user failed")?;
    Ok(u64::from_le_bytes(bytes))
}

fn write_u64_user(
    task: &crate::task::Task,
    user_addr: usize,
    value: u64,
) -> Result<(), &'static str> {
    copy_to_user(task, user_addr, &value.to_le_bytes()).map_err(|_| "copy_to_user failed")
}

fn write_u32_user(
    task: &crate::task::Task,
    user_addr: usize,
    value: u32,
) -> Result<(), &'static str> {
    copy_to_user(task, user_addr, &value.to_le_bytes()).map_err(|_| "copy_to_user failed")
}

fn read_ntdll_context(
    task: &crate::task::Task,
    user_addr: usize,
) -> Result<peb::NtdllContext, &'static str> {
    let mut bytes = [0u8; core::mem::size_of::<peb::NtdllContext>()];
    copy_from_user(task, user_addr, &mut bytes).map_err(|_| "copy_from_user failed")?;

    let context = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const peb::NtdllContext) };
    Ok(context)
}

fn read_u16_user(task: &crate::task::Task, user_addr: usize) -> Result<u16, &'static str> {
    let mut bytes = [0u8; 2];
    copy_from_user(task, user_addr, &mut bytes).map_err(|_| "copy_from_user failed")?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32_user(task: &crate::task::Task, user_addr: usize) -> Result<u32, &'static str> {
    let mut bytes = [0u8; 4];
    copy_from_user(task, user_addr, &mut bytes).map_err(|_| "copy_from_user failed")?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64_user_raw(task: &crate::task::Task, user_addr: usize) -> Result<u64, &'static str> {
    let mut bytes = [0u8; 8];
    copy_from_user(task, user_addr, &mut bytes).map_err(|_| "copy_from_user failed")?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_unicode_string_from_user(
    task: &crate::task::Task,
    ptr: usize,
) -> Result<String, &'static str> {
    if ptr == 0 {
        return Err("null UNICODE_STRING pointer");
    }

    let length = read_u16_user(task, ptr)? as usize;
    let buffer_ptr = read_u64_user_raw(task, ptr + 0x08)? as usize;

    if buffer_ptr == 0 || length == 0 {
        return Ok(String::new());
    }

    let char_count = length / 2;
    let mut utf16_bytes = Vec::new();
    utf16_bytes.resize(length, 0);
    copy_from_user(task, buffer_ptr, &mut utf16_bytes).map_err(|_| "copy_from_user failed")?;

    let utf16: Vec<u16> = (0..char_count)
        .map(|i| u16::from_le_bytes([utf16_bytes[i * 2], utf16_bytes[i * 2 + 1]]))
        .collect();

    String::from_utf16(&utf16).map_err(|_| "invalid UTF-16 in UNICODE_STRING")
}

struct NtObjectAttributes {
    root_directory: u32,
    object_name: String,
    attributes: u32,
}

fn read_object_attributes_from_user(
    task: &crate::task::Task,
    ptr: usize,
) -> Result<NtObjectAttributes, &'static str> {
    if ptr == 0 {
        return Err("null OBJECT_ATTRIBUTES pointer");
    }

    let root_directory = read_u32_user(task, ptr + 0x08)?;
    let object_name_ptr = read_u64_user_raw(task, ptr + 0x10)? as usize;
    let attributes = read_u32_user(task, ptr + 0x18)?;

    let object_name = read_unicode_string_from_user(task, object_name_ptr)?;

    Ok(NtObjectAttributes {
        root_directory,
        object_name,
        attributes,
    })
}

fn register_windows_abi() {
    register_abi!(WindowsAarch64Abi);
}

late_initcall!(register_windows_abi);
