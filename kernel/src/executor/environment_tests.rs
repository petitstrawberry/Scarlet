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
            crate::vm::vmem::PhysicalMemoryArea::new(paddr, paddr + 4095),
            MemoryArea::new(0x40_0000, 0x40_0fff),
            0,
            false,
            None,
        ))?;
        *image.name.write() = "prepared-image".to_string();
        image.text_size.store(4096, Ordering::Relaxed);
        image.vcpu.lock().set_pc(0x40_0000);
        tf.set_pc(0x40_0000);
        if envp.contains(&"GAIN=1") {
            image.credentials.write().set_principals(crate::shield::identity::Identities::all(
                crate::shield::identity::Principal(42))).unwrap();
        }
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

#[test_case]
fn mount_namespace_preserves_environment_through_fork_exec_and_failed_exec() {
    let (task, original, environment, _) = fixture();
    seal_execution(&task);
    let backing = Arc::new(VfsManager::new());
    backing
        .create_file("/shared-data", FileType::RegularFile)
        .unwrap();
    original.create_dir("/mounted").unwrap();
    original.bind_mount_from(&backing, "/", "/mounted").unwrap();
    crate::abi::linux::generic::proc::unshare_filesystem(&task, 0x20000).unwrap();
    let separated = task.get_vfs().unwrap();
    let namespace = task.mount_namespace.read().clone();
    assert!(!Arc::ptr_eq(&separated.view(), &original.view()));
    assert!(Arc::ptr_eq(
        task.execution_environment.read().as_ref().unwrap(),
        &environment
    ));
    assert_eq!(separated.get_cwd_path(), "/work");
    separated.unmount("/mounted").unwrap();
    assert!(separated.open("/mounted/shared-data", 0).is_err());
    assert!(original.open("/mounted/shared-data", 0).is_ok());
    task.credentials.write().set_principals(crate::shield::identity::Identities::all(
        crate::shield::identity::Principal(1234))).unwrap();
    let child = task.clone_task(crate::task::CloneFlags::default()).unwrap();
    assert_execution_sealed(&child);
    assert_eq!(child.credentials.read().principals().active,
        crate::shield::identity::Principal(1234));
    assert!(Arc::ptr_eq(&namespace, &child.mount_namespace.read()));
    assert!(Arc::ptr_eq(
        &separated.view(),
        &child.get_vfs().unwrap().view()
    ));
    let mut tf = Trapframe::new();
    assert!(
        TransparentExecutor::execute_with_abi(
            "/txn-image",
            &["/txn-image"],
            &["FAIL=1"],
            TestImageAbi::name(),
            &task,
            &mut tf
        )
        .is_err()
    );
    assert_execution_sealed(&task);
    assert!(Arc::ptr_eq(&namespace, &task.mount_namespace.read()));
    assert!(Arc::ptr_eq(&separated, &task.get_vfs().unwrap()));
    TransparentExecutor::execute_with_abi(
        "/txn-image",
        &["/txn-image"],
        &[],
        TestImageAbi::name(),
        &task,
        &mut tf,
    )
    .unwrap();
    task.finish_exec_dispatch();
    assert_execution_sealed(&task);
    assert!(Arc::ptr_eq(&namespace, &task.mount_namespace.read()));
    assert!(Arc::ptr_eq(
        &separated.view(),
        &task.get_vfs().unwrap().view()
    ));
    assert!(Arc::ptr_eq(
        task.execution_environment.read().as_ref().unwrap(),
        &environment
    ));
    assert_eq!(task.get_vfs().unwrap().get_cwd_path(), "/work");
    assert!(
        task.get_vfs()
            .unwrap()
            .open("/mounted/shared-data", 0)
            .is_err()
    );
    assert!(original.open("/mounted/shared-data", 0).is_ok());
}

#[test_case]
fn mount_namespace_explicit_spawn_inherits_current_mappings_and_repeated_splits() {
    let (source, original, env, _) = fixture();
    seal_execution(&source);
    let file = original.open("/txn-image", 0).unwrap();
    let (namespace, fs) = source.prepare_mount_namespace().unwrap();
    source.install_mount_namespace(namespace.clone(), fs.clone());
    source.credentials.write().set_principals(crate::shield::identity::Identities::all(
        crate::shield::identity::Principal(1234))).unwrap();
    let child = new_user_task("namespace-spawn".into(), 0);
    let mut tf = Trapframe::new();
    TransparentExecutor::execute_in_environment(
        env.clone(),
        &file,
        &["/txn-image"],
        &[],
        "/work",
        &[],
        &source,
        &child,
        &mut tf,
    )
    .unwrap();
    child.finish_exec_dispatch();
    assert_execution_sealed(&child);
    assert_eq!(child.credentials.read().principals().active,
        crate::shield::identity::Principal(1234));
    assert!(Arc::ptr_eq(&namespace, &child.mount_namespace.read()));
    assert!(Arc::ptr_eq(&fs.view(), &child.get_vfs().unwrap().view()));
    assert!(Arc::ptr_eq(
        child.execution_environment.read().as_ref().unwrap(),
        &env
    ));
    let backing = Arc::new(VfsManager::new());
    backing.create_file("/new", FileType::RegularFile).unwrap();
    fs.create_dir("/private").unwrap();
    fs.bind_mount_from(&backing, "/", "/private").unwrap();
    let (next, second) = child.prepare_mount_namespace().unwrap();
    child.install_mount_namespace(next, second.clone());
    assert!(second.open("/private/new", 0).is_ok());
    second.unmount("/private").unwrap();
    assert!(fs.open("/private/new", 0).is_ok());
    assert!(second.open("/private/new", 0).is_err());
    assert!(original.open("/private/new", 0).is_err());
}

#[test_case]
fn unshare_rejects_unknown_flags_without_changes_and_private_mounts_stay_private() {
    use crate::abi::linux::generic::{
        errno, fs::private_mount_propagation, proc::unshare_filesystem,
    };
    let (task, fs, _, _) = fixture();
    let namespace = task.mount_namespace.read().clone();
    assert_eq!(
        unshare_filesystem(&task, 0x20000 | 0x40000000),
        Err(errno::EOPNOTSUPP)
    );
    assert!(Arc::ptr_eq(&namespace, &task.mount_namespace.read()));
    assert!(Arc::ptr_eq(&fs, &task.get_vfs().unwrap()));
    unshare_filesystem(&task, 0x200).unwrap();
    assert!(!Arc::ptr_eq(&fs, &task.get_vfs().unwrap()));
    assert!(Arc::ptr_eq(&fs.view(), &task.get_vfs().unwrap().view()));
    task.get_vfs().unwrap().set_cwd_by_path("/").unwrap();
    assert_eq!(fs.get_cwd_path(), "/work");
    assert_eq!(private_mount_propagation(&task, "/", 0x84000), Ok(()));
    assert_eq!(private_mount_propagation(&task, "/", 0x40000), Ok(()));
    assert_eq!(
        private_mount_propagation(&task, "/", 0x100000),
        Err(errno::EOPNOTSUPP)
    );
    assert_eq!(
        private_mount_propagation(&task, "/work", 0x80000),
        Err(errno::EINVAL)
    );
    assert_eq!(
        private_mount_propagation(&task, "/absent", 0x80000),
        Err(errno::ENOENT)
    );
}

struct ExecutionTestRule;
impl crate::shield::execution::CallConstraint for ExecutionTestRule {
    fn evaluate(&self, _: &crate::shield::execution::Invocation) -> crate::shield::execution::Verdict {
        crate::shield::execution::Verdict::ABORT
    }
}
fn seal_execution(task: &Task) {
    let mut state = task.execution_restrictions.write();
    state.seal_authority();
    state.append(Arc::new(ExecutionTestRule), 1).unwrap();
}
fn assert_execution_sealed(task: &Task) {
    let state = task.execution_restrictions.read();
    assert!(state.no_new_authority());
    assert_eq!(state.evaluate(&crate::shield::execution::Invocation {
        domain: "test", number: 0, instruction: 0, arguments: [0;6],
    }), Some(crate::shield::execution::Denial::AbortProcess));
}

#[test_case]
fn mount_restrictions_gate_exec_scripts_and_file_derived_identity_changes() {
    use crate::fs::{drivers::tmpfs::TmpFS, vfs_v2::mount_tree::MountRestrictions as R};
    let (task, fs, _, _) = fixture();
    fs.create_dir("/restricted").unwrap();
    fs.mount(TmpFS::new(0), "/restricted", R::NO_EXECUTE).unwrap();
    fs.create_file("/restricted/script", FileType::RegularFile).unwrap();
    let file = fs.open("/restricted/script", 2).unwrap();
    file.as_stream().unwrap().write(b"#!/txn-image\n").unwrap();
    let mut tf = Trapframe::new();
    assert!(matches!(TransparentExecutor::execute_binary("/restricted/script", &["script"], &[], &task, &mut tf),
        Err(ExecutorError::ExecutionFailed(message)) if message == "execution disabled on mount"));
    fs.unmount("/restricted").unwrap();
    fs.mount(TmpFS::new(0), "/restricted", R::NO_PRIVILEGE_GAIN).unwrap();
    fs.create_file("/restricted/image", FileType::RegularFile).unwrap();
    let credentials = task.credentials.read().clone();
    assert!(TransparentExecutor::execute_with_abi("/restricted/image", &["image"], &["GAIN=1"],
        TestImageAbi::name(), &task, &mut tf).is_err());
    assert_eq!(*task.credentials.read(), credentials);
    assert_eq!(task.vm_manager.memmap_len(), 0);
    TransparentExecutor::execute_with_abi("/restricted/image", &["image"], &[],
        TestImageAbi::name(), &task, &mut tf).unwrap();
    assert_eq!(*task.credentials.read(), credentials);
}

#[test_case]
fn linux_tmpfs_mount_enforces_flags_options_and_native_mount_authority() {
    use crate::abi::linux::generic::{fs::mount_tmpfs, errno};
    let (task, fs, _, _) = fixture();
    fs.create_dir("/apex").unwrap();
    assert_eq!(mount_tmpfs(&task, "/apex", "tmpfs", 14, "mode=0755,uid=0,gid=0,size=1M"), Ok(()));
    assert_eq!(fs.resolve_path("/apex").unwrap().1.restrictions.bits(), 14);
    for (flags, options, error) in [(15, "", errno::EOPNOTSUPP), (14, "unknown=1", errno::EOPNOTSUPP),
        (14, "uid=1000", errno::EOPNOTSUPP), (14, "mode=xyz", errno::EINVAL), (14, "size=invalid", errno::EINVAL)] {
        assert_eq!(mount_tmpfs(&task, "/apex", "tmpfs", flags, options), Err(error));
        assert_eq!(fs.resolve_path("/apex").unwrap().1.restrictions.bits(), 14);
    }
    task.credentials.write().set_authority(0, 0).unwrap();
    assert_eq!(mount_tmpfs(&task, "/apex", "tmpfs", 14, ""), Err(errno::EPERM));
}

#[test_case]
fn unsupported_shared_mount_reports_operation_error_with_null_filesystem_type() {
    use crate::abi::linux::generic::{fs::sys_mount, errno, LinuxAbi};
    let task = Arc::new(new_user_task("shared-mount-request".into(), 0));
    let path = 0x20000000;
    task.allocate_stack_pages(path, 1).unwrap();
    crate::library::std::usercopy::copy_to_user(&task, path, b"/\0").unwrap();
    let mut frame = Trapframe::new();
    frame.set_arg(1, path);
    frame.set_arg(3, 0x104000); // MS_SHARED | MS_REC; type/source/data are NULL.
    crate::task::set_mock_current_task(task);
    let result = sys_mount(&mut LinuxAbi::default(), &mut frame);
    crate::task::clear_mock_current_task();
    assert_eq!(result, errno::to_result(errno::EOPNOTSUPP));
}
