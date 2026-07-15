use alloc::{boxed::Box, format, string::ToString, sync::Arc, vec::Vec};
use core::sync::atomic::Ordering;

use crate::abi::linux::generic;
use crate::{
    abi::{AbiModule, EventProcessOutcome},
    arch::{self, IntRegisters},
    fs::{
        FileSystemError, FileSystemErrorKind, SeekFrom, VfsManager, drivers::overlayfs::OverlayFS,
    },
    late_initcall, register_abi,
    task::elf_loader::{
        ExecutionMode, LoadStrategy, LoadTarget, analyze_and_load_elf_with_strategy,
    },
    vm::setup_user_stack,
};

pub mod signal;

const TRACE_KVM_COMPAT_SYSCALLS: bool = false;

fn trace_kvm_compat_syscall(trapframe: &crate::arch::Trapframe, syscall_number: usize) {
    if !TRACE_KVM_COMPAT_SYSCALLS {
        return;
    }

    let Some(task) = crate::task::mytask() else {
        return;
    };
    let task_name = task.name.read();
    if !task_name.contains("firectl") && !task_name.contains("firecracker") {
        return;
    }

    match syscall_number {
        40 | 56 | 57 | 95 | 97 | 124 | 135 | 167 | 172 | 178 | 220 | 221 | 222 | 226 | 260
        | 278 | 434 => {
            crate::println!(
                "[linux-aarch64-trace] task={} pid={} syscall={} args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
                task_name.as_str(),
                task.get_id(),
                syscall_number,
                trapframe.get_arg(0),
                trapframe.get_arg(1),
                trapframe.get_arg(2),
                trapframe.get_arg(3),
                trapframe.get_arg(4),
                trapframe.get_arg(5),
            );
        }
        _ => {}
    }
}

#[derive(Clone)]
pub struct LinuxAarch64Abi(pub generic::LinuxAbi);

impl Default for LinuxAarch64Abi {
    fn default() -> Self {
        Self(generic::LinuxAbi::default())
    }
}

impl core::ops::Deref for LinuxAarch64Abi {
    type Target = generic::LinuxAbi;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl core::ops::DerefMut for LinuxAarch64Abi {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AbiModule for LinuxAarch64Abi {
    fn name() -> &'static str {
        "linux-aarch64"
    }

    fn get_name(&self) -> alloc::string::String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> alloc::boxed::Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }

    fn handle_syscall(
        &mut self,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<usize, &'static str> {
        let syscall_number = trapframe.get_syscall_number();
        if syscall_number == 0 {
            return Err("Invalid syscall number");
        }

        trace_kvm_compat_syscall(trapframe, syscall_number);

        if let Some(result) =
            generic::dispatch_common_syscall(&mut self.0, trapframe, syscall_number)
        {
            return Ok(result);
        }

        if let Some(result) = signal::dispatch_arch_syscall(self, trapframe, syscall_number) {
            return Ok(result);
        }

        crate::println!("Invalid Syscall number: {}", syscall_number);
        Err("Invalid syscall number")
    }

    fn handle_event(
        &mut self,
        event: crate::ipc::Event,
        target_task_id: u32,
    ) -> Result<EventProcessOutcome, &'static str> {
        generic::signal::handle_event_for_task(
            &self.0,
            &event,
            target_task_id,
            signal::setup_signal_handler,
        )
    }

    fn on_task_cloned(
        &mut self,
        _parent_task: &crate::task::Task,
        _child_task: &crate::task::Task,
        _flags: crate::task::CloneFlags,
    ) -> Result<(), &'static str> {
        if !_flags.is_set(crate::task::CloneFlagsDef::Files) {
            self.0.unshare_fd_table();
        }

        let mut ts = self.0.thread_state.clone();
        let parent_tgid = ts.tgid;
        let is_thread = ts.pending_clone_is_thread;

        ts.tgid = if is_thread {
            if parent_tgid != 0 {
                parent_tgid
            } else {
                _parent_task.get_id()
            }
        } else {
            0
        };

        ts.pending_clone_is_thread = false;
        self.0.thread_state = ts;
        Ok(())
    }

    fn on_task_exit(&mut self, task: &crate::task::Task) {
        if let Some(ptr) = self.0.thread_state.clear_child_tid_ptr {
            if let Some(paddr) = task.vm_manager.translate_to_kva(ptr) {
                unsafe {
                    *(paddr as *mut i32) = 0;
                }
                let _ = generic::futex::wake_address(ptr, 1);
            }
        }
    }

    fn get_task_namespace(&self) -> Arc<crate::task::namespace::TaskNamespace> {
        self.0.namespace.clone()
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        let magic_score = match file_object.as_file() {
            Some(file_obj) => {
                let mut magic_buffer = [0u8; 4];
                file_obj.seek(SeekFrom::Start(0)).ok();
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        if magic_buffer == [0x7F, b'E', b'L', b'F'] {
                            35
                        } else {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
            None => return None,
        };

        let mut confidence = magic_score;

        if let Some(file_obj) = file_object.as_file() {
            let mut osabi_buffer = [0u8; 1];
            file_obj.seek(SeekFrom::Start(7)).ok();
            match file_obj.read(&mut osabi_buffer) {
                Ok(bytes_read) if bytes_read == 1 => {
                    if osabi_buffer[0] == 0 {
                        confidence += 50;
                    }
                }
                _ => return None,
            }
        } else {
            return None;
        }

        if file_path.contains("linux") || file_path.ends_with(".linux") {
            confidence += 20;
        } else if file_path.ends_with(".elf") {
            confidence += 5;
        }

        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence += 15;
            }
        }

        Some(confidence.min(100))
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &crate::task::Task,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<(), &'static str> {
        match file_object.as_file() {
            Some(file_obj) => {
                task.text_size.store(0, Ordering::SeqCst);
                task.data_size.store(0, Ordering::SeqCst);
                task.stack_size.store(0, Ordering::SeqCst);
                task.brk
                    .store(usize::MAX, core::sync::atomic::Ordering::SeqCst);

                match analyze_and_load_elf_with_strategy(
                    file_obj,
                    task,
                    &LoadStrategy {
                        choose_base_address: |target, needs_relocation| match (
                            target,
                            needs_relocation,
                        ) {
                            (LoadTarget::MainProgram, false) => 0,
                            (LoadTarget::MainProgram, true) => 0x40000000,
                            (LoadTarget::Interpreter, _) => 0x40000000,
                            (LoadTarget::SharedLib, _) => 0x50000000,
                        },
                        resolve_interpreter: |requested| {
                            requested.map(|path| {
                                if path.starts_with("/lib/ld-") || path.starts_with("/lib64/ld-") {
                                    format!("/scarlet/system/linux-aarch64{}", path)
                                } else {
                                    path.to_string()
                                }
                            })
                        },
                    },
                ) {
                    Ok(load_result) => {
                        *task.name.write() =
                            argv.get(0).map_or("linux".to_string(), |s| s.to_string());

                        let mut root_page_table =
                            arch::vm::get_root_pagetable(task.vm_manager.get_asid()).unwrap();
                        root_page_table.unmap_all();
                        drop(root_page_table);
                        arch::vm::setup_trampoline_for_user(&task.vm_manager);
                        let (_, stack_top) = setup_user_stack(task);
                        let mut sp = stack_top as usize;

                        if let ExecutionMode::Dynamic { .. } = &load_result.mode {
                            sp -= 96;
                            unsafe {
                                let kaddr = task.vm_manager.translate_to_kva(sp).unwrap();
                                let slice = core::slice::from_raw_parts_mut(kaddr as *mut u8, 96);
                                slice.fill(0);
                            }
                        }

                        let mut arg_vaddrs: Vec<u64> = Vec::new();
                        for &arg in argv.iter() {
                            let len = arg.len() + 1;
                            sp -= len;
                            let vaddr = sp;
                            unsafe {
                                let kaddr = task.vm_manager.translate_to_kva(vaddr).unwrap();
                                let slice = core::slice::from_raw_parts_mut(kaddr as *mut u8, len);
                                slice[..len - 1].copy_from_slice(arg.as_bytes());
                                slice[len - 1] = 0;
                            }
                            arg_vaddrs.push(vaddr as u64);
                        }

                        let mut env_vaddrs: Vec<u64> = Vec::new();
                        for &env in envp.iter() {
                            let len = env.len() + 1;
                            sp -= len;
                            let vaddr = sp;
                            unsafe {
                                let kaddr = task.vm_manager.translate_to_kva(vaddr).unwrap();
                                let slice = core::slice::from_raw_parts_mut(kaddr as *mut u8, len);
                                slice[..len - 1].copy_from_slice(env.as_bytes());
                                slice[len - 1] = 0;
                            }
                            env_vaddrs.push(vaddr as u64);
                        }

                        sp = sp & !0xF;

                        use crate::task::elf_loader::build_auxiliary_vector;
                        let auxv = build_auxiliary_vector(&load_result);

                        let auxv_size = auxv.len() * 16;
                        let envp_size = (env_vaddrs.len() + 1) * 8;
                        let argv_size = (arg_vaddrs.len() + 1) * 8;
                        let argc_size = 8;
                        let total_structured_size = auxv_size + envp_size + argv_size + argc_size;

                        let aligned_size = (total_structured_size + 15) & !15;
                        sp -= aligned_size;
                        let final_sp = sp;
                        let mut current_pos = final_sp;

                        let argc = argv.len() as u64;
                        unsafe {
                            *(task.vm_manager.translate_to_kva(current_pos).unwrap() as *mut u64) =
                                argc;
                        }
                        current_pos += 8;

                        for &arg_vaddr in arg_vaddrs.iter() {
                            unsafe {
                                *(task.vm_manager.translate_to_kva(current_pos).unwrap()
                                    as *mut u64) = arg_vaddr;
                            }
                            current_pos += 8;
                        }
                        unsafe {
                            *(task.vm_manager.translate_to_kva(current_pos).unwrap() as *mut u64) =
                                0;
                        }
                        current_pos += 8;

                        for &env_vaddr in env_vaddrs.iter() {
                            unsafe {
                                *(task.vm_manager.translate_to_kva(current_pos).unwrap()
                                    as *mut u64) = env_vaddr;
                            }
                            current_pos += 8;
                        }
                        unsafe {
                            *(task.vm_manager.translate_to_kva(current_pos).unwrap() as *mut u64) =
                                0;
                        }
                        current_pos += 8;

                        for auxv_entry in auxv.iter() {
                            unsafe {
                                let paddr = task.vm_manager.translate_to_kva(current_pos).unwrap()
                                    as *mut u64;
                                *paddr = auxv_entry.a_type;
                                *(paddr.add(1)) = auxv_entry.a_val;
                            }
                            current_pos += 16;
                        }

                        sp = final_sp;

                        task.set_entry_point(load_result.entry_point as usize);
                        task.vcpu.lock().iregs = IntRegisters::new();
                        task.vcpu.lock().set_sp(sp);

                        trapframe.regs = task.vcpu.lock().iregs;
                        trapframe.set_pc(load_result.entry_point);

                        task.vcpu.lock().switch(trapframe);
                        Ok(())
                    }
                    Err(e) => {
                        crate::println!("Failed to load Linux ELF binary: {:?}", e);
                        Err("Failed to load Linux ELF binary")
                    }
                }
            }
            None => Err("Invalid file object type for Linux binary execution"),
        }
    }

    fn get_default_cwd(&self) -> &str {
        "/"
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
                    "Failed to create overlay filesystem for Linux ABI: {}",
                    e.message
                );
                return Err("Failed to create Linux overlay environment");
            }
        };
        match target_vfs.mount(fs, "/", 0) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!(
                    "Failed to create cross-VFS overlay for Linux ABI: {}",
                    e.message
                );
                Err("Failed to create Linux overlay environment")
            }
        }
    }

    fn setup_shared_resources(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
    ) -> Result<(), &'static str> {
        match create_dir_if_not_exists(target_vfs, "/home") {
            Ok(()) => {}
            Err(_e) => {
                return Err("Failed to create /home directory for Linux");
            }
        }

        let _ = target_vfs.bind_mount_from(base_vfs, "/home", "/home");

        match create_dir_if_not_exists(target_vfs, "/data") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to create /data directory for Linux: {}", e.message);
                return Err("Failed to create /data directory for Linux");
            }
        }

        let _ = target_vfs.bind_mount_from(base_vfs, "/data/shared", "/data/shared");

        match create_dir_if_not_exists(target_vfs, "/dev") {
            Ok(()) => {}
            Err(_e) => {
                return Err("Failed to create /dev directory for Linux");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/dev", "/dev") {
            Ok(()) => {}
            Err(_e) => {
                return Err("Failed to bind mount /dev for Linux");
            }
        }
        if base_vfs.resolve_path("/dev/pts/ptmx").is_ok() {
            let _ = create_dir_if_not_exists(target_vfs, "/dev/pts");
            if target_vfs
                .bind_mount_from(base_vfs, "/dev/pts", "/dev/pts")
                .is_err()
            {
                crate::println!("Failed to bind mount /dev/pts for Linux");
            }
        }

        match create_dir_if_not_exists(target_vfs, "/tmp") {
            Ok(()) => {}
            Err(_e) => {
                return Err("Failed to create /tmp directory for Linux");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/tmp", "/tmp") {
            Ok(()) => {}
            Err(_e) => {
                return Err("Failed to bind mount /tmp for Linux");
            }
        }

        match create_dir_if_not_exists(target_vfs, "/scarlet") {
            Ok(()) => {}
            Err(_e) => {
                return Err("Failed to create /scarlet directory for Linux");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/", "/scarlet") {
            Ok(()) => Ok(()),
            Err(_e) => Err("Failed to bind mount native Scarlet root to /scarlet for Linux"),
        }
    }

    fn initialize_from_existing_handles(
        &mut self,
        _task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        self.0.init_std_fds(0, 1, 2);
        self.0.thread_state.tgid = _task.get_id();
        Ok(())
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

fn create_dir_if_not_exists(vfs: &Arc<VfsManager>, path: &str) -> Result<(), FileSystemError> {
    match vfs.create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if e.kind == FileSystemErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

fn register_linux_abi() {
    register_abi!(LinuxAarch64Abi);
}

late_initcall!(register_linux_abi);
