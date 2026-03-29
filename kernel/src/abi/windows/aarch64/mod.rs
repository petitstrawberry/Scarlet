mod syscall_table;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use spin::Mutex;

use crate::abi::AbiModule;
use crate::abi::windows::error::{
    STATUS_INVALID_HANDLE, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED,
    STATUS_OBJECT_NAME_NOT_FOUND, STATUS_SUCCESS,
};
use crate::abi::windows::object::{
    NtFileObject, NtObject, NtObjectTable, NtSectionObject, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE,
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
use crate::task::pe_loader::{find_export_by_name, load_pe_from_bytes, load_pe_into_task};
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

const NTDLL_IMAGE_BASE: u64 = 0x0000_0000_1800_0000;
const NT_SYSCALL_NT_CONTINUE: u16 = 0x43;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_WRITECOPY: u32 = 0x08;
const PAGE_EXECUTE: u32 = 0x10;
const PAGE_EXECUTE_READ: u32 = 0x20;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
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
        let syscall_number = ((trapframe.esr_el1 >> 5) & 0xFFFF) as u16;
        let mut args = [0usize; 8];
        args.copy_from_slice(&trapframe.regs.reg[0..8]);

        let ret = match syscall_number {
            0x02 => status(STATUS_SUCCESS),
            0x04 => self.nt_allocate_virtual_memory(args),
            0x05 => self.nt_free_virtual_memory(args),
            0x06 => self.nt_close(args[0] as u32),
            0x09 => status(STATUS_SUCCESS),
            0x0A => self.nt_read_file(args),
            0x10 => status(STATUS_SUCCESS),
            0x11 => status(STATUS_SUCCESS),
            0x18 => self.nt_create_file(args),
            0x19 => status(STATUS_SUCCESS),
            0x1A => self.nt_read_file(args),
            0x1B => self.nt_write_file(args),
            0x20 => status(STATUS_SUCCESS),
            0x23 => self.nt_query_virtual_memory(args),
            0x25 => status(STATUS_NOT_IMPLEMENTED),
            0x26 => status(STATUS_SUCCESS),
            0x28 => self.nt_map_view_of_section(args),
            NT_SYSCALL_NT_CONTINUE => self.nt_continue(trapframe, args),
            0x29 => status(STATUS_SUCCESS),
            0x2A => self.nt_unmap_view_of_section(args),
            0x2E => self.nt_terminate_process(args),
            0x35 => status(STATUS_SUCCESS),
            0x36 => self.rtl_allocate_heap(args),
            0x37 => self.nt_open_section(args),
            0x3A => self.nt_write_virtual_memory(args),
            0x3C => self.nt_get_current_process_id(),
            0x3D => status(STATUS_SUCCESS),
            0x3E => status(STATUS_SUCCESS),
            0x3F => status(STATUS_SUCCESS),
            0x40 => status(STATUS_SUCCESS),
            0x4A => self.nt_create_section(args),
            0x50 => self.nt_protect_virtual_memory(args),
            0x51 => self.nt_query_section(args),
            0x55 => status(STATUS_SUCCESS),
            0x56 => status(STATUS_SUCCESS),
            0x5A => status(STATUS_SUCCESS),
            0x5B => status(STATUS_SUCCESS),
            _ => status(STATUS_NOT_IMPLEMENTED),
        };

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
        let pe = load_pe_into_task(file, task, None).map_err(|_| "Failed to load PE image")?;
        let ntdll = self.load_ntdll(task)?;

        let ldr_initialize_thunk = ntdll
            .ldr_initialize_thunk
            .unwrap_or(ntdll.entry_point)
            .max(ntdll.entry_point);

        let heap_size = 1024 * 1024;
        let heap_base = task
            .vm_manager
            .find_unmapped_area(heap_size, PAGE_SIZE)
            .ok_or("No free address for Windows heap")?;
        let heap_pages = heap_size / PAGE_SIZE;
        task.allocate_data_pages(heap_base, heap_pages)?;

        let mut state = self.state.lock();
        state.object_table = NtObjectTable::new();
        state.object_table.register_console_pseudo_handles();
        state.heap_base = heap_base;
        state.heap_current = heap_base;
        state.heap_end = heap_base + heap_size;
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

        let env = peb::initialize_process_environment(
            task,
            pe.image_base,
            pe.entry_point,
            pe.image_size,
            heap_base as u64,
            peb::NtDllData {
                ldr_data_address: 0,
                image_entry_address: 0,
                ntdll_entry_address: ntdll.image_base,
            },
            ldr_initialize_thunk,
            sp as u64,
        )?;
        state.peb_address = env.peb_address;
        state.teb_address = env.teb_address;
        state.context_address = env.context_address;
        drop(state);

        crate::println!("[windows-aarch64] PE import resolution is not implemented yet");

        let context_ptr = env.context_address;
        trapframe.elr = ldr_initialize_thunk;
        trapframe.sp = context_ptr;
        trapframe.regs.reg[0] = context_ptr as usize;
        trapframe.regs.reg[1] = 0;
        trapframe.tpidr_el0 = env.teb_address;
        trapframe.regs.reg[18] = env.teb_address as usize; // x18 = TEB on ARM64 Windows

        task.set_entry_point(ldr_initialize_thunk as usize);
        {
            let mut vcpu = task.vcpu.lock();
            vcpu.set_sp(context_ptr as usize);
            vcpu.set_tpidr_el0(env.teb_address);
            vcpu.set_pc(ldr_initialize_thunk);
        }

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
    fn nt_allocate_virtual_memory(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let base_ptr = args[0];
        let size_ptr = args[1];
        let requested_base = read_u64_user(task, base_ptr).unwrap_or(0) as usize;
        let requested_size = read_u64_user(task, size_ptr).unwrap_or(PAGE_SIZE as u64) as usize;
        if requested_size == 0 {
            return status(STATUS_INVALID_PARAMETER);
        }

        let size = align_up(requested_size, PAGE_SIZE);
        let base = if requested_base == 0 {
            match task.vm_manager.find_unmapped_area(size, PAGE_SIZE) {
                Some(addr) => addr,
                None => return status(STATUS_INVALID_PARAMETER),
            }
        } else {
            align_down(requested_base, PAGE_SIZE)
        };

        let pages = size / PAGE_SIZE;
        if task.allocate_data_pages(base, pages).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        if write_u64_user(task, base_ptr, base as u64).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }
        if write_u64_user(task, size_ptr, size as u64).is_err() {
            return status(STATUS_INVALID_PARAMETER);
        }

        status(STATUS_SUCCESS)
    }

    fn nt_free_virtual_memory(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let base_ptr = args[0];
        let size_ptr = args[1];
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
        let buffer_ptr = args[1];
        let length = args[2];
        if length == 0 {
            return status(STATUS_SUCCESS);
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
        let buffer_ptr = args[1];
        let length = args[2];
        if length == 0 {
            return status(STATUS_SUCCESS);
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

    fn nt_terminate_process(&mut self, args: [usize; 8]) -> usize {
        let task = match mytask() {
            Some(task) => task,
            None => return status(STATUS_INVALID_PARAMETER),
        };
        let exit_status = args[1] as i32;
        task.exit(exit_status);
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
            return status(STATUS_NOT_IMPLEMENTED);
        }

        let map = match task.vm_manager.search_memory_map(base_address) {
            Some(map) => map,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        const MEMORY_BASIC_INFORMATION_SIZE: usize = 48;
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

        let ldr_initialize_thunk = find_export_by_name(ntdll_slice, "LdrInitializeThunk")
            .map(|rva| load.image_base + rva as u64);

        Ok(NtDllLoadResult {
            image_base: load.image_base,
            entry_point: load.entry_point,
            ldr_initialize_thunk,
        })
    }
}

struct NtDllLoadResult {
    image_base: u64,
    entry_point: u64,
    ldr_initialize_thunk: Option<u64>,
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
