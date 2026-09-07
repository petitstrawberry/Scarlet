//! Environment mapping, exec transaction, and cwd/handle inheritance tests.

use super::{
    environment::Environment,
    executor::{ExecutorError, HandleMapping, TransparentExecutor},
};
use crate::{
    abi::{AbiModule, AbiRegistry},
    arch::Trapframe,
    fs::{FileType, VfsManager},
    mem::page::ContiguousPages,
    object::{KernelObject, handle::SpecialSemantics},
    task::{Task, new_user_task},
    vm::vmem::{MemoryArea, VirtualMemoryMap},
};
use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
};
use core::sync::atomic::Ordering;

#[derive(Default, Clone)]
struct TestImageAbi;
impl AbiModule for TestImageAbi {
    fn name() -> &'static str {
        "environment-test-image"
    }
    fn get_name(&self) -> String {
        Self::name().to_string()
    }
    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }
    fn handle_syscall(&mut self, _: &mut Trapframe) -> Result<usize, &'static str> {
        Err("test ABI")
    }
    fn can_execute_binary(
        &self,
        _: &KernelObject,
        path: &str,
        _: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        (path == "/txn-image").then_some(100)
    }
    fn execute_binary(
        &self,
        _: &KernelObject,
        _: &[&str],
        envp: &[&str],
        image: &Task,
        tf: &mut Trapframe,
    ) -> Result<(), &'static str> {
        let page = ContiguousPages::new(1).ok_or("test allocation failed")?;
        let paddr = page.as_paddr();
        image.page_allocations.write().push(page);
        image.vm_manager.add_memory_map(VirtualMemoryMap::new(
            MemoryArea::new(paddr, paddr + 4095),
            MemoryArea::new(0x40_0000, 0x40_0fff),
            0,
            false,
            None,
        ))?;
        *image.name.write() = "prepared-image".to_string();
        image.text_size.store(4096, Ordering::Relaxed);
        image.vcpu.lock().set_pc(0x40_0000);
        tf.set_pc(0x40_0000);
        if envp.contains(&"FAIL=1") {
            return Err("deliberate failure after mapping");
        }
        Ok(())
    }
}

fn fixture() -> (Task, Arc<VfsManager>, Arc<Environment>, u32) {
    AbiRegistry::register::<TestImageAbi>();
    let fs = Arc::new(VfsManager::new());
    fs.create_dir("/work").unwrap();
    fs.create_file("/txn-image", FileType::RegularFile).unwrap();
    fs.set_cwd_by_path("/work").unwrap();
    let env = Environment::new();
    env.set_root(TestImageAbi::name().to_string(), fs.view())
        .unwrap();
    env.seal().unwrap();
    let task = new_user_task("original".to_string(), 0);
    task.set_vfs(fs.clone());
    *task.execution_environment.write() = Some(env.clone());
    task.install_exec_abi(Box::new(TestImageAbi));
    task.finish_exec_dispatch();
    let handle = task
        .handle_table
        .insert(fs.open("/txn-image", 0).unwrap())
        .unwrap();
    let mut metadata = task.handle_table.get_metadata(handle).unwrap();
    metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
    task.handle_table.update_metadata(handle, metadata).unwrap();
    (task, fs, env, handle)
}

#[test_case]
fn environment_seal_freezes_slots_not_file_data() {
    let (_, fs, env, _) = fixture();
    assert!(
        env.set_root(TestImageAbi::name().to_string(), fs.view())
            .is_err()
    );
    assert!(env.remove_root(TestImageAbi::name()).is_err());
    assert!(env.root("absent").is_err());
    fs.create_file("/after-seal", FileType::RegularFile)
        .unwrap();
    assert!(
        VfsManager::from_view(env.root(TestImageAbi::name()).unwrap())
            .open("/after-seal", 0)
            .is_ok()
    );

    let builder = Environment::new();
    builder
        .set_root(TestImageAbi::name().to_string(), fs.view())
        .unwrap();
    assert!(
        builder
            .set_root(TestImageAbi::name().to_string(), fs.view())
            .is_err()
    );
    assert!(builder.root(TestImageAbi::name()).is_err());
    assert!(
        builder
            .set_root("no-such-abi".to_string(), fs.view())
            .is_err()
    );
    assert!(Environment::new().seal().is_err());
}

#[test_case]
fn failed_exec_keeps_memory_handles_environment_and_cwd() {
    let (task, fs, env, handle) = fixture();
    task.text_size.store(123, Ordering::Relaxed);
    task.brk.store(456, Ordering::Relaxed);
    let mut tf = Trapframe::new();
    tf.set_pc(0x1234);
    let before_asid = task.vm_manager.get_asid();
    let error = TransparentExecutor::execute_with_abi(
        "/txn-image",
        &["/txn-image"],
        &["FAIL=1"],
        TestImageAbi::name(),
        &task,
        &mut tf,
    )
    .unwrap_err();
    assert!(matches!(error, ExecutorError::ExecutionFailed(_)));
    assert_eq!(task.vm_manager.memmap_len(), 0);
    assert_eq!(task.vm_manager.get_asid(), before_asid);
    assert!(task.page_allocations.read().is_empty());
    assert_eq!(task.text_size.load(Ordering::Relaxed), 123);
    assert_eq!(task.brk.load(Ordering::Relaxed), 456);
    assert_eq!(task.name.read().as_str(), "original");
    assert_eq!(tf.get_current_pc(), 0x1234);
    assert!(task.handle_table.is_valid_handle(handle));
    assert!(Arc::ptr_eq(&task.get_vfs().unwrap(), &fs));
    assert_eq!(fs.get_cwd_path(), "/work");
    assert!(Arc::ptr_eq(
        task.execution_environment.read().as_ref().unwrap(),
        &env
    ));
}

#[test_case]
fn same_abi_exec_preserves_cwd_and_applies_cloexec_only_on_success() {
    let (task, _, _, handle) = fixture();
    let mut tf = Trapframe::new();
    TransparentExecutor::execute_with_abi(
        "/txn-image",
        &["/txn-image"],
        &[],
        TestImageAbi::name(),
        &task,
        &mut tf,
    )
    .unwrap();
    assert_eq!(task.get_vfs().unwrap().get_cwd_path(), "/work");
    assert!(!task.handle_table.is_valid_handle(handle));
    assert_eq!(task.vm_manager.memmap_len(), 1);
    assert_eq!(tf.get_current_pc(), 0x40_0000);
    task.finish_exec_dispatch();
}

#[test_case]
fn explicit_exec_checks_seal_and_abi_and_uses_only_mapped_handles() {
    let (task, source, original, handle) = fixture();
    let target = Arc::new(VfsManager::new());
    target.create_dir("/next").unwrap();
    let env = Environment::new();
    env.set_root(TestImageAbi::name().to_string(), target.view())
        .unwrap();
    let executable = source.open("/txn-image", 0).unwrap();
    let mut tf = Trapframe::new();
    let mappings = [HandleMapping {
        source: handle,
        target: 7,
    }];
    assert!(
        TransparentExecutor::execute_in_environment(
            env.clone(),
            &executable,
            &["/txn-image"],
            &[],
            "/next",
            &mappings,
            &task,
            &task,
            &mut tf
        )
        .is_err()
    );
    assert!(Arc::ptr_eq(
        task.execution_environment.read().as_ref().unwrap(),
        &original
    ));
    env.seal().unwrap();
    let invalid_mappings = [
        HandleMapping {
            source: handle,
            target: 7,
        },
        HandleMapping {
            source: handle,
            target: 7,
        },
    ];
    assert!(
        TransparentExecutor::execute_in_environment(
            env.clone(),
            &executable,
            &["/txn-image"],
            &[],
            "/next",
            &invalid_mappings,
            &task,
            &task,
            &mut tf
        )
        .is_err()
    );
    assert!(task.handle_table.is_valid_handle(handle));
    TransparentExecutor::execute_in_environment(
        env.clone(),
        &executable,
        &["/txn-image"],
        &[],
        "/next",
        &mappings,
        &task,
        &task,
        &mut tf,
    )
    .unwrap();
    assert_eq!(task.handle_table.active_handles(), alloc::vec![7]);
    assert!(matches!(
        task.handle_table.get_metadata(7).unwrap().special_semantics,
        Some(SpecialSemantics::CloseOnExec)
    ));
    assert_eq!(task.get_vfs().unwrap().get_cwd_path(), "/next");
    assert!(Arc::ptr_eq(
        task.execution_environment.read().as_ref().unwrap(),
        &env
    ));
    assert!(!task.bootstrap_environment.load(Ordering::Acquire));
    task.finish_exec_dispatch();
}

#[test_case]
fn copied_fs_contexts_share_mounts_but_not_cwd() {
    let (_, fs, _, _) = fixture();
    let child = VfsManager::clone_with_shared_mount_namespace(&fs);
    child.set_cwd_by_path("/").unwrap();
    assert_eq!(fs.get_cwd_path(), "/work");
    assert!(Arc::ptr_eq(&fs.view(), &child.view()));
    fs.create_dir("/mounted").unwrap();
    fs.mount(crate::fs::drivers::tmpfs::TmpFS::new(0), "/mounted", 0)
        .unwrap();
    assert!(child.resolve_path("/mounted").is_ok());
}

#[test_case]
fn environment_clone_keeps_external_binds_and_isolates_mount_topology() {
    let (_, fs, env, _) = fixture();
    let backing = Arc::new(VfsManager::new());
    backing.create_file("/data", FileType::RegularFile).unwrap();
    fs.create_dir("/shared").unwrap();
    fs.bind_mount_from(&backing, "/", "/shared").unwrap();
    drop(backing);
    let copy = env.clone_mount_namespaces().unwrap();
    let cloned = VfsManager::from_view(copy.root(TestImageAbi::name()).unwrap());
    assert!(!Arc::ptr_eq(&fs.view(), &cloned.view()));
    assert!(cloned.open("/shared/data", 0).is_ok());
    cloned.unmount("/shared").unwrap();
    assert!(cloned.open("/shared/data", 0).is_err());
    assert!(fs.open("/shared/data", 0).is_ok());
}
