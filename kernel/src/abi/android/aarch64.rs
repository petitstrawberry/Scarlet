use crate::{
    abi::{
        AbiModule, EventProcessOutcome,
        linux::{aarch64::LinuxAarch64Abi, generic::LinuxAbi},
    },
    arch::Trapframe,
    object::KernelObject,
    task::{CloneFlags, Task, namespace::TaskNamespace},
};
use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
};

/// Keep one Linux state/implementation for syscalls, signals, clone and exec.
/// The outer ABI owns Android detection, registration and device translations.
#[derive(Clone)]
pub struct AndroidAarch64Abi {
    linux: LinuxAarch64Abi,
}

impl Default for AndroidAarch64Abi {
    fn default() -> Self {
        Self {
            linux: LinuxAarch64Abi(LinuxAbi::with_extensions(&super::AndroidExtensions)),
        }
    }
}

impl AbiModule for AndroidAarch64Abi {
    fn name() -> &'static str {
        "android-aarch64"
    }
    fn get_name(&self) -> String {
        Self::name().to_string()
    }
    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }
    fn base_abi(&self) -> Option<&dyn AbiModule> {
        Some(&self.linux)
    }

    fn can_execute_binary(
        &self,
        file: &KernelObject,
        _path: &str,
        _current: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        super::detect::is_android_aarch64(file).then_some(100)
    }

    fn handle_syscall(&mut self, tf: &mut Trapframe) -> Result<usize, &'static str> {
        self.linux.handle_syscall(tf)
    }
    fn handle_event(
        &mut self,
        event: crate::ipc::Event,
        target: usize,
    ) -> Result<EventProcessOutcome, &'static str> {
        self.linux.handle_event(event, target)
    }
    fn on_task_cloned(
        &mut self,
        parent: &Task,
        child: &Task,
        flags: CloneFlags,
    ) -> Result<(), &'static str> {
        self.linux.on_task_cloned(parent, child, flags)
    }
    fn on_task_exit(&mut self, task: &Task) {
        self.linux.on_task_exit(task);
    }
    fn on_process_exit(&mut self, task: &Task) {
        self.linux.on_process_exit(task);
    }
    fn on_exec_commit(&mut self, task: &Task) {
        self.linux.on_exec_commit(task);
    }
    fn get_task_namespace(&self) -> Arc<TaskNamespace> {
        self.linux.get_task_namespace()
    }

    fn execute_binary(
        &self,
        file: &KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &Task,
        tf: &mut Trapframe,
    ) -> Result<(), &'static str> {
        self.linux.execute_binary(file, argv, envp, task, tf)
    }
    fn prepare_exec_handles(
        &mut self,
        source: Option<&dyn AbiModule>,
        image: &Task,
    ) -> Result<(), &'static str> {
        self.linux.prepare_exec_handles(source, image)
    }
    fn initialize_from_existing_handles(&mut self, task: &Task) -> Result<(), &'static str> {
        self.linux.initialize_from_existing_handles(task)
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

fn register_android_abi() {
    crate::register_abi!(AndroidAarch64Abi);
}
crate::late_initcall!(register_android_abi);
