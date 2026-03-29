mod syscall_table;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use spin::Mutex;

use crate::abi::AbiModule;
use crate::abi::windows::error::{
    STATUS_INVALID_HANDLE, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED, STATUS_SUCCESS,
};
use crate::abi::windows::object::{
    NtFileObject, NtObject, NtObjectTable, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use crate::abi::windows::peb;
use crate::arch::Trapframe;
use crate::environment::PAGE_SIZE;
use crate::fs::SeekFrom;
use crate::late_initcall;
use crate::library::std::string::parse_c_string_from_userspace;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::register_abi;
use crate::task::mytask;
use crate::task::namespace::TaskNamespace;
use crate::task::pe_loader::headers::{
    DOS_MAGIC, DosHeader, IMAGE_FILE_MACHINE_ARM64, PE_SIGNATURE, PE32PLUS_MAGIC,
};
use crate::task::pe_loader::load_pe_into_task;
use crate::vm;

pub const ABI_NAME: &str = "windows-aarch64";

#[derive(Clone, Default)]
struct WindowsProcessState {
    object_table: NtObjectTable,
    peb_address: u64,
    teb_address: u64,
    heap_base: usize,
    heap_current: usize,
    heap_end: usize,
}

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
            0x25 => status(STATUS_NOT_IMPLEMENTED),
            0x26 => status(STATUS_SUCCESS),
            0x29 => status(STATUS_SUCCESS),
            0x2E => self.nt_terminate_process(args),
            0x35 => status(STATUS_SUCCESS),
            0x36 => self.rtl_allocate_heap(args),
            0x37 => status(STATUS_SUCCESS),
            0x3A => status(STATUS_SUCCESS),
            0x3C => self.nt_get_current_process_id(),
            0x3D => status(STATUS_SUCCESS),
            0x3E => status(STATUS_SUCCESS),
            0x3F => status(STATUS_SUCCESS),
            0x40 => status(STATUS_SUCCESS),
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

        let env =
            peb::initialize_process_environment(task, trapframe, pe.image_base, heap_base as u64)?;
        state.peb_address = env.peb_address;
        state.teb_address = env.teb_address;
        drop(state);

        crate::println!("[windows-aarch64] PE import resolution is not implemented yet");

        trapframe.elr = pe.entry_point;
        trapframe.sp = sp as u64;
        trapframe.tpidr_el0 = env.teb_address;

        task.set_entry_point(pe.entry_point as usize);
        task.vcpu.lock().set_sp(sp);
        task.vcpu.lock().set_tpidr_el0(env.teb_address);

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
        let path_ptr = args[1];
        let open_flags = args[2] as u32;

        let path = match parse_c_string_from_userspace(task, path_ptr, 1024) {
            Ok(path) => path,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };

        let vfs = match get_vfs_for_task(task) {
            Some(vfs) => vfs,
            None => return status(STATUS_INVALID_PARAMETER),
        };

        let object = match vfs.open(&path, open_flags) {
            Ok(obj) => obj,
            Err(_) => return status(STATUS_INVALID_PARAMETER),
        };

        let file = match object {
            crate::object::KernelObject::File(file) => file,
            _ => return status(STATUS_INVALID_PARAMETER),
        };

        let mut state = self.state.lock();
        let handle = state.object_table.insert(NtObject::File(NtFileObject {
            file,
            path: Some(path),
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

fn get_vfs_for_task(task: &crate::task::Task) -> Option<Arc<crate::fs::VfsManager>> {
    task.get_vfs()
        .or_else(crate::fs::vfs_v2::manager::get_global_vfs_manager_safe)
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

fn register_windows_abi() {
    register_abi!(WindowsAarch64Abi);
}

late_initcall!(register_windows_abi);
