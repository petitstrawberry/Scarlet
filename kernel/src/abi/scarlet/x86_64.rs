//! Scarlet Native ABI Module (x86_64)

use alloc::{
    boxed::Box,
    collections::btree_map::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};

use crate::{
    arch::{self, Trapframe},
    early_initcall,
    ipc::event::{Event, EventContent, ProcessControlType},
    register_abi,
    syscall::syscall_handler,
    task::elf_loader::{LoadStrategy, LoadTarget, analyze_and_load_elf_with_strategy},
    vm,
};

use crate::abi::AbiModule;

const MAX_PENDING_EVENTS: usize = 1024;

pub type EventHandler = usize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventHandlerEntry {
    pub handler: EventHandler,
    pub synchronous: bool,
}

#[derive(Debug, Clone, Default)]
pub struct EventMask {
    pub blocked_process_control: u32,
    pub blocked_notifications: u64,
    pub blocked_namespaces: Vec<String>,
    pub block_all: bool,
}

impl EventMask {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn block_all(&mut self) {
        self.block_all = true;
    }

    pub fn unblock_all(&mut self) {
        self.block_all = false;
        self.blocked_process_control = 0;
        self.blocked_notifications = 0;
        self.blocked_namespaces.clear();
    }

    pub fn block_process_control(&mut self, _ptype: ProcessControlType) {}

    pub fn unblock_process_control(&mut self, _ptype: ProcessControlType) {}

    pub fn is_blocked(&self, _content: &EventContent) -> bool {
        self.block_all
    }
}

#[derive(Clone)]
pub struct ScarletAbi {
    pub tls_pointer: Option<usize>,
    pub clear_child_tid_ptr: Option<usize>,
    pub event_handlers: BTreeMap<u8, EventHandlerEntry>,
    pub default_event_handler: Option<EventHandlerEntry>,
    pub event_mask: EventMask,
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
    pub fn new() -> Self {
        Self::default()
    }

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

    pub fn unregister_event_handler(&mut self, content_type: u8) {
        self.event_handlers.remove(&content_type);
    }

    pub fn set_default_event_handler(&mut self, handler: EventHandler, synchronous: bool) {
        self.default_event_handler = Some(EventHandlerEntry {
            handler,
            synchronous,
        });
    }

    pub fn process_pending_events(
        &mut self,
        _task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        while let Some(_event) = self.pending_events.pop() {}
        Ok(())
    }

    /// Restore context saved by `invoke_user_handler` and resume
    /// the interrupted code.
    ///
    /// Signal frame layout on x86_64:
    ///   [sp]:       saved r15
    ///   [sp + 8]:   saved r14
    ///   [sp + 16]:  saved r13
    ///   [sp + 24]:  saved r12
    ///   [sp + 32]:  saved rbp
    ///   [sp + 40]:  saved rbx
    ///   [sp + 48]:  saved r11
    ///   [sp + 56]:  saved r10
    ///   [sp + 64]:  saved r9
    ///   [sp + 72]:  saved r8
    ///   [sp + 80]:  saved rax
    ///   [sp + 88]:  saved rcx
    ///   [sp + 96]:  saved rdx
    ///   [sp + 104]: saved rsi
    ///   [sp + 112]: saved rdi
    ///   [sp + 120]: saved rip
    ///   [sp + 128]: saved rflags
    ///   [sp + 136]: saved rsp
    pub fn event_return(
        trapframe: &mut crate::arch::Trapframe,
        task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        let frame_base = trapframe.rsp as usize;

        unsafe {
            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.r15 = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 8)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.r14 = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 16)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.r13 = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 24)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.r12 = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 32)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.rbp = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 40)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.rbx = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 48)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.r11 = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 56)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.r10 = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 64)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.r9 = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 72)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.r8 = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 80)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.rax = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 88)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.rcx = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 96)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.rdx = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 104)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.rsi = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 112)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.regs.rdi = *(paddr as *const usize);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 120)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.rip = *(paddr as *const u64);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 128)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.rflags = *(paddr as *const u64);

            let paddr = task
                .vm_manager
                .translate_vaddr(frame_base + 136)
                .ok_or("Failed to translate signal frame address")?;
            trapframe.rsp = *(paddr as *const u64);
        }

        Ok(())
    }
}

impl AbiModule for ScarletAbi {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        "scarlet"
    }

    fn get_name(&self) -> String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }

    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        syscall_handler(trapframe)
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &crate::task::Task,
        trapframe: &mut Trapframe,
    ) -> Result<(), &'static str> {
        match file_object.as_file() {
            Some(file_obj) => {
                task.text_size
                    .store(0, core::sync::atomic::Ordering::SeqCst);
                task.data_size
                    .store(0, core::sync::atomic::Ordering::SeqCst);
                task.stack_size
                    .store(0, core::sync::atomic::Ordering::SeqCst);
                task.brk
                    .store(usize::MAX, core::sync::atomic::Ordering::SeqCst);

                let strategy = LoadStrategy {
                    choose_base_address: |target, needs_relocation| match (target, needs_relocation)
                    {
                        (LoadTarget::MainProgram, false) => 0,
                        (LoadTarget::MainProgram, true) => 0x10000,
                        (LoadTarget::Interpreter, _) => 0x40000000,
                        (LoadTarget::SharedLib, _) => 0x50000000,
                    },
                    resolve_interpreter: |requested| requested.map(|s| s.to_string()),
                };

                match analyze_and_load_elf_with_strategy(file_obj, task, &strategy) {
                    Ok(elf_result) => {
                        *task.name.write() = argv
                            .get(0)
                            .map_or("Unnamed Task".to_string(), |s| s.to_string());

                        let root_page_table =
                            arch::vm::get_root_pagetable(task.vm_manager.get_asid()).unwrap();
                        root_page_table.unmap_all();

                        arch::vm::setup_trampoline_for_user(&task.vm_manager);
                        let stack_pointer = vm::setup_user_stack(task).1;

                        task.set_entry_point(elf_result.entry_point as usize);
                        task.vcpu.lock().reset_iregs();
                        task.vcpu.lock().set_sp(stack_pointer);

                        let argc = argv.len();
                        let argv_ptr = 0usize;

                        task.vcpu.lock().iregs.rdi = argc;
                        task.vcpu.lock().iregs.rsi = argv_ptr;

                        task.vcpu.lock().switch(trapframe);
                        Ok(())
                    }
                    Err(_e) => Err("Failed to load ELF binary"),
                }
            }
            None => Err("Invalid file object type for binary execution"),
        }
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        let mut confidence = 0u8;

        if let Some(file) = file_object.as_file() {
            let mut buf = [0u8; 4];
            use crate::object::capability::StreamOps;
            let _ = file.read_at(0, &mut buf);

            if &buf == b"\x7fELF" {
                confidence += 30;
            }

            if file.read_at(4, &mut buf[..1]).is_ok() && buf[0] == 2 {
                confidence += 15;
            }

            let mut e_machine = [0u8; 2];
            if file.read_at(18, &mut e_machine).is_ok() {
                let machine = u16::from_le_bytes(e_machine);
                if machine == 0x3D {
                    confidence += 30;
                }
            }
        }

        if file_path.contains("scarlet") {
            confidence = confidence.saturating_add(15);
        }

        if let Some(abi) = current_abi {
            if abi.get_name() == Self::name() {
                confidence = confidence.saturating_add(25);
            }
        }

        Some(confidence.min(100))
    }

    fn choose_load_address(&self, elf_type: u16, target: LoadTarget) -> Option<u64> {
        const ET_DYN: u16 = 3;

        if elf_type == ET_DYN {
            match target {
                LoadTarget::MainProgram => Some(0x0000_0040_0000_0000),
                LoadTarget::Interpreter => Some(0x0000_0070_0000_0000),
                LoadTarget::SharedLib => None,
            }
        } else {
            None
        }
    }

    fn set_tls_pointer(&mut self, ptr: usize) {
        self.tls_pointer = Some(ptr);
    }

    fn get_tls_pointer(&self) -> Option<usize> {
        self.tls_pointer
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

fn abi_scarlet_x86_64_init() {
    register_abi!(ScarletAbi);
}

early_initcall!(abi_scarlet_x86_64_init);
