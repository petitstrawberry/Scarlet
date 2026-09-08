//! Capability-scoped view construction and explicit Environment execution.
//! All returned management handles are close-on-exec. Current-environment
//! queries never manufacture management authority.

use super::{
    environment::{Environment, EnvironmentHandle, ViewHandle},
    executor::{HandleMapping, TransparentExecutor},
};
use crate::{
    arch::Trapframe,
    fs::{
        VfsManager,
        vfs_v2::{drivers::overlayfs::OverlayFS, manager::VfsView},
    },
    library::std::usercopy::copy_from_user,
    object::{
        KernelObject,
        handle::{HandleTable, SpecialSemantics},
    },
    task::{Task, mytask},
};
use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::sync::atomic::Ordering;
use scarlet_abi::{data_model::AbiDataModel, environment::EnvironmentExec};

type Result<T> = core::result::Result<T, &'static str>;

// Serializes dependency checks with view composition. Filesystem mutations do
// not hold an IRQ guard while resolving paths or performing backing-store I/O.
static VIEW_COMPOSITION: crate::sync::Mutex<()> = crate::sync::Mutex::new(());

pub(crate) fn lock_composition() -> crate::sync::MutexGuard<'static, ()> {
    VIEW_COMPOSITION.lock()
}

pub(crate) fn is_root_path(fs: &VfsManager, path: &str) -> bool {
    let root = fs.mount_tree.root_mount.read().clone();
    fs.resolve_path(path)
        .is_ok_and(|(entry, mount)| Arc::ptr_eq(&entry, &root.root) && Arc::ptr_eq(&mount, &root))
}

fn text_arg(task: &Task, address: usize) -> Result<String> {
    let mut bytes = Vec::new();
    for offset in 0..4096 {
        let mut byte = [0u8];
        copy_from_user(
            task,
            address.checked_add(offset).ok_or("invalid pointer")?,
            &mut byte,
        )
        .map_err(|_| "invalid pointer")?;
        if byte[0] == 0 {
            return String::from_utf8(bytes).map_err(|_| "invalid UTF-8");
        }
        bytes.push(byte[0]);
    }
    Err("string too long")
}

fn object(task: &Task, handle: usize) -> Result<KernelObject> {
    let handle = u32::try_from(handle).map_err(|_| "invalid handle")?;
    task.handle_table.get(handle).ok_or("invalid handle")
}

fn view(task: &Task, handle: usize, admin: bool) -> Result<Arc<VfsManager>> {
    match object(task, handle)? {
        KernelObject::VfsView(cap) if !admin || cap.writable => Ok(VfsManager::from_view(cap.view)),
        _ => Err("view capability required"),
    }
}
fn environment(task: &Task, handle: usize, admin: bool) -> Result<Arc<Environment>> {
    match object(task, handle)? {
        KernelObject::Environment(cap) if !admin || cap.writable => Ok(cap.environment),
        _ => Err("Environment capability required"),
    }
}
fn insert(task: &Task, object: KernelObject) -> Result<usize> {
    let handle = task.handle_table.insert(object)?;
    let mut metadata = task
        .handle_table
        .get_metadata(handle)
        .ok_or("missing handle metadata")?;
    metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
    task.handle_table.update_metadata(handle, metadata)?;
    Ok(handle as usize)
}
fn insert_view(task: &Task, fs: Arc<VfsManager>, writable: bool) -> Result<usize> {
    insert(
        task,
        KernelObject::VfsView(ViewHandle {
            view: fs.view(),
            writable,
        }),
    )
}
pub(crate) fn may_manage_view(task: &Task, view: &Arc<VfsView>) -> bool {
    if task.bootstrap_environment.load(Ordering::Acquire) {
        return true;
    }
    task.handle_table.active_handles().iter().any(|&h| {
        matches!(task.handle_table.get(h), Some(KernelObject::VfsView(cap)) if cap.writable && Arc::ptr_eq(&cap.view, view))
    })
}
fn may_create_view(task: &Task) -> bool {
    task.bootstrap_environment.load(Ordering::Acquire) || task.handle_table.active_handles().iter().any(|&h| {
        matches!(task.handle_table.get(h), Some(KernelObject::VfsView(cap)) if cap.writable)
    })
}
fn require_builder(task: &Task) -> Result<()> {
    if may_create_view(task) {
        Ok(())
    } else {
        Err("view construction capability required")
    }
}

// Retain dependency edges as weak references for the lifetime of a composed
// view. An unmounted backing edge can conservatively prevent a reverse bind
// until a new view is built, but never keeps a dead view alive.
fn depends_on(source: &Arc<VfsView>, target: &Arc<VfsView>, visited: &mut Vec<usize>) -> bool {
    if Arc::ptr_eq(source, target) {
        return true;
    }
    let id = Arc::as_ptr(source) as usize;
    if visited.contains(&id) {
        return false;
    }
    visited.push(id);
    let edges = source.dependencies.read().clone();
    edges
        .into_iter()
        .filter_map(|edge| edge.upgrade())
        .any(|edge| depends_on(&edge, target, visited))
}
fn add_dependency(target: &Arc<VfsView>, source: &Arc<VfsView>) -> Result<()> {
    if depends_on(source, target, &mut Vec::new()) {
        return Err("cyclic backing views");
    }
    target.dependencies.write().push(Arc::downgrade(source));
    Ok(())
}

macro_rules! handler {
    ($name:ident, |$task:ident, $tf:ident| $body:block) => {
        pub fn $name($tf: &mut Trapframe) -> usize {
            let $task = mytask().unwrap();
            $tf.increment_pc_next(&$task);
            (|| -> Result<usize> { $body })().unwrap_or(usize::MAX)
        }
    };
}

handler!(sys_environment_create, |task, tf| {
    insert(
        &task,
        KernelObject::Environment(EnvironmentHandle {
            environment: Environment::new(),
            writable: true,
        }),
    )
});
handler!(sys_environment_set_root, |task, tf| {
    let env = environment(&task, tf.get_arg(0), true)?;
    let abi = text_arg(&task, tf.get_arg(1))?;
    let fs = view(&task, tf.get_arg(2), false)?;
    env.set_root(abi, fs.view())?;
    Ok(0)
});
handler!(sys_environment_remove_root, |task, tf| {
    environment(&task, tf.get_arg(0), true)?.remove_root(&text_arg(&task, tf.get_arg(1))?)?;
    Ok(0)
});
handler!(sys_environment_seal, |task, tf| {
    environment(&task, tf.get_arg(0), true)?.seal()?;
    Ok(0)
});
handler!(sys_environment_current, |task, tf| {
    let env = task
        .execution_environment
        .read()
        .clone()
        .ok_or("bootstrap has no Environment")?;
    insert(
        &task,
        KernelObject::Environment(EnvironmentHandle {
            environment: env,
            writable: false,
        }),
    )
});
handler!(sys_environment_get_root, |task, tf| {
    let env = environment(&task, tf.get_arg(0), false)?;
    let root = env.root(&text_arg(&task, tf.get_arg(1))?)?;
    insert_view(&task, VfsManager::from_view(root), false)
});

handler!(sys_vfs_view_create, |task, tf| {
    require_builder(&task)?;
    let fstype = text_arg(&task, tf.get_arg(0))?;
    let options = if tf.get_arg(1) == 0 {
        String::new()
    } else {
        text_arg(&task, tf.get_arg(1))?
    };
    if fstype == "overlay" {
        return Err("use the scoped overlay API");
    }
    let fs = crate::fs::get_fs_driver_manager()
        .create_from_option_string(&fstype, &options)
        .map_err(|_| "filesystem creation failed")?;
    insert_view(&task, Arc::new(VfsManager::new_with_root(fs)), true)
});
handler!(sys_vfs_view_current, |task, tf| {
    let fs = task.get_vfs().ok_or("no current view")?;
    let flags = tf.get_arg(0);
    if flags > 1 || (flags == 1 && !may_manage_view(&task, &fs.view())) {
        return Err("view administration denied");
    }
    insert_view(&task, fs, flags == 1)
});
handler!(sys_vfs_view_clone, |task, tf| {
    let fs = view(&task, tf.get_arg(0), true)?;
    let _guard = VIEW_COMPOSITION.lock();
    let copy = VfsManager::clone_mount_namespace_deep(&fs).map_err(|_| "view clone failed")?;
    insert_view(&task, copy, true)
});
handler!(sys_vfs_view_root, |task, tf| {
    let fs = view(&task, tf.get_arg(0), true)?;
    let path = text_arg(&task, tf.get_arg(1))?;
    let _guard = VIEW_COMPOSITION.lock();
    let rooted = VfsManager::view_rooted_at(&fs, &path).map_err(|_| "invalid root directory")?;
    add_dependency(&rooted.view(), &fs.view())?;
    insert_view(&task, rooted, true)
});
handler!(sys_vfs_view_open, |task, tf| {
    let fs = view(&task, tf.get_arg(0), false)?;
    let path = text_arg(&task, tf.get_arg(1))?;
    let flags = u32::try_from(tf.get_arg(2)).map_err(|_| "invalid open flags")?;
    let access_mode = match flags & 3 {
        0 => crate::object::handle::AccessMode::ReadOnly,
        1 => crate::object::handle::AccessMode::WriteOnly,
        2 => crate::object::handle::AccessMode::ReadWrite,
        _ => return Err("invalid access mode"),
    };
    let file = fs.open(&path, flags).map_err(|_| "view open failed")?;
    if flags & 0x200 != 0 {
        if flags & 3 == 0 {
            return Err("truncate requires write access");
        }
        file.as_file()
            .ok_or("not a file")?
            .truncate(0)
            .map_err(|_| "truncate failed")?;
    }
    if flags & 0x400 != 0 {
        file.as_file()
            .ok_or("not a file")?
            .seek(crate::fs::SeekFrom::End(0))
            .map_err(|_| "seek failed")?;
    }
    task.handle_table
        .insert_with_metadata(
            file,
            crate::object::handle::HandleMetadata {
                handle_type: crate::object::handle::HandleType::Regular,
                access_mode,
                special_semantics: Some(SpecialSemantics::CloseOnExec),
            },
        )
        .map(|handle| handle as usize)
});
handler!(sys_vfs_view_mkdir, |task, tf| {
    let fs = view(&task, tf.get_arg(0), false)?;
    let path = text_arg(&task, tf.get_arg(1))?;
    fs.create_dir(&path)
        .map_err(|_| "create directory failed")?;
    Ok(0)
});
handler!(sys_vfs_view_mount, |task, tf| {
    let fs = view(&task, tf.get_arg(0), true)?;
    let path = text_arg(&task, tf.get_arg(1))?;
    let fstype = text_arg(&task, tf.get_arg(2))?;
    let options = if tf.get_arg(3) == 0 {
        String::new()
    } else {
        text_arg(&task, tf.get_arg(3))?
    };
    if is_root_path(&fs, &path) || fstype == "overlay" {
        return Err("construct a new root view instead");
    }
    let _guard = VIEW_COMPOSITION.lock();
    let filesystem = crate::fs::get_fs_driver_manager()
        .create_from_option_string(&fstype, &options)
        .map_err(|_| "filesystem creation failed")?;
    fs.mount(filesystem, &path, 0)
        .map_err(|_| "view mount failed")?;
    Ok(0)
});
handler!(sys_vfs_view_bind, |task, tf| {
    let target = view(&task, tf.get_arg(0), true)?;
    let target_path = text_arg(&task, tf.get_arg(1))?;
    let source = view(&task, tf.get_arg(2), false)?;
    let source_path = text_arg(&task, tf.get_arg(3))?;
    if is_root_path(&target, &target_path) {
        return Err("construct a new root view instead");
    }
    let _guard = VIEW_COMPOSITION.lock();
    if !Arc::ptr_eq(&target.view(), &source.view()) {
        add_dependency(&target.view(), &source.view())?;
    }
    target
        .bind_mount_from(&source, &source_path, &target_path)
        .map_err(|_| "view bind failed")?;
    Ok(0)
});
handler!(sys_vfs_view_overlay, |task, tf| {
    require_builder(&task)?;
    let lower = view(&task, tf.get_arg(0), false)?;
    let lower_path = text_arg(&task, tf.get_arg(1))?;
    let upper = if tf.get_arg(2) == usize::MAX {
        None
    } else {
        Some(view(&task, tf.get_arg(2), false)?)
    };
    let upper_path = if upper.is_some() {
        text_arg(&task, tf.get_arg(3))?
    } else {
        String::new()
    };
    let _guard = VIEW_COMPOSITION.lock();
    // The new root has no incoming mount edges; adding these backing references
    // cannot create an ownership cycle. Reverse binds are checked separately.
    let overlay = OverlayFS::new_from_paths_and_vfs(
        upper.as_ref().map(|vfs| (vfs, upper_path.as_str())),
        alloc::vec![(&lower, lower_path.as_str())],
        "overlay",
    )
    .map_err(|_| "overlay creation failed")?;
    let fs = Arc::new(VfsManager::new_with_root(overlay));
    add_dependency(&fs.view(), &lower.view())?;
    if let Some(upper) = upper {
        add_dependency(&fs.view(), &upper.view())?;
    }
    insert_view(&task, fs, true)
});
handler!(sys_vfs_view_unmount, |task, tf| {
    let fs = view(&task, tf.get_arg(0), true)?;
    let path = text_arg(&task, tf.get_arg(1))?;
    if is_root_path(&fs, &path) {
        return Err("cannot unmount view root");
    }
    let _guard = VIEW_COMPOSITION.lock();
    fs.unmount(&path).map_err(|_| "unmount failed")?;
    Ok(0)
});

struct ExecOptions {
    argv: Vec<String>,
    envp: Vec<String>,
    cwd: String,
    handles: Vec<HandleMapping>,
}
fn string_array(task: &Task, address: usize) -> Result<Vec<String>> {
    let mut result = Vec::new();
    if address == 0 {
        return Ok(result);
    }
    let model = AbiDataModel::NATIVE;
    let base = model
        .user_address(address as u64)
        .map_err(|_| "invalid array")?;
    for index in 0..=256 {
        let mut bytes = [0u8; core::mem::size_of::<usize>()];
        let slot = base
            .element(index, model.word_width.bytes() as u64)
            .and_then(|address| address.to_usize())
            .map_err(|_| "invalid array")?;
        copy_from_user(task, slot, &mut bytes).map_err(|_| "invalid array")?;
        let ptr = model
            .read_word(&bytes, 0)
            .and_then(|word| model.user_address(word.unsigned()))
            .and_then(|address| address.to_usize())
            .map_err(|_| "invalid array")?;
        if ptr == 0 {
            return Ok(result);
        }
        if index == 256 {
            return Err("too many strings");
        }
        result.push(text_arg(task, ptr)?);
    }
    Err("unterminated string array")
}
fn exec_options(task: &Task, address: usize) -> Result<ExecOptions> {
    // Native callers currently share the kernel's model. A compatibility ABI
    // must select its own model before reaching this boundary.
    let model = AbiDataModel::NATIVE;
    let mut bytes = [0u8; EnvironmentExec::MAX_ENCODED_SIZE];
    let bytes = &mut bytes[..EnvironmentExec::encoded_size(model)];
    copy_from_user(task, address, bytes).map_err(|_| "invalid options")?;
    let options = EnvironmentExec::decode(model, bytes).map_err(|_| "unsupported exec options")?;
    if options.handle_count > HandleTable::MAX_HANDLES as u64 {
        return Err("too many handles");
    }
    let mut handles = Vec::new();
    for index in 0..options.handle_count {
        let mut entry = [0u8; 8];
        let address = options
            .handles
            .element(index, 8)
            .and_then(|address| address.to_usize())
            .map_err(|_| "invalid handle map")?;
        copy_from_user(task, address, &mut entry).map_err(|_| "invalid handle map")?;
        handles.push(HandleMapping {
            source: model
                .read_u32(&entry, 0)
                .map_err(|_| "invalid handle map")?,
            target: model
                .read_u32(&entry, 4)
                .map_err(|_| "invalid handle map")?,
        });
    }
    Ok(ExecOptions {
        argv: string_array(task, options.argv.to_usize().map_err(|_| "invalid argv")?)?,
        envp: string_array(task, options.envp.to_usize().map_err(|_| "invalid envp")?)?,
        cwd: if options.cwd.is_null() {
            "/".to_string()
        } else {
            text_arg(task, options.cwd.to_usize().map_err(|_| "invalid cwd")?)?
        },
        handles,
    })
}
handler!(sys_environment_exec, |task, tf| {
    let environment = environment(&task, tf.get_arg(0), false)?;
    let executable = object(&task, tf.get_arg(1))?;
    let options = exec_options(&task, tf.get_arg(2))?;
    let abi = if tf.get_arg(3) == 0 {
        None
    } else {
        Some(text_arg(&task, tf.get_arg(3))?)
    };
    let argv: Vec<_> = options.argv.iter().map(String::as_str).collect();
    let envp: Vec<_> = options.envp.iter().map(String::as_str).collect();
    TransparentExecutor::execute_in_environment_with_abi(
        environment,
        &executable,
        &argv,
        &envp,
        &options.cwd,
        &options.handles,
        &task,
        &task,
        tf,
        abi.as_deref(),
    )
    .map_err(|error| {
        crate::println!("Environment exec: {}", error);
        "Environment exec failed"
    })?;
    Ok(tf.get_return_value())
});
handler!(sys_environment_spawn, |task, tf| {
    let environment = environment(&task, tf.get_arg(0), false)?;
    let executable = object(&task, tf.get_arg(1))?;
    let options = exec_options(&task, tf.get_arg(2))?;
    let abi = if tf.get_arg(3) == 0 {
        None
    } else {
        Some(text_arg(&task, tf.get_arg(3))?)
    };
    let argv: Vec<_> = options.argv.iter().map(String::as_str).collect();
    let envp: Vec<_> = options.envp.iter().map(String::as_str).collect();
    let mut child = Task::new_with_namespace(
        "spawn".to_string(),
        0,
        crate::task::TaskType::User,
        task.get_namespace(),
    );
    child.max_stack_size = task.max_stack_size;
    child.max_data_size = task.max_data_size;
    child.max_text_size = task.max_text_size;
    child.set_session_id(task.get_session_id());
    child.set_process_group_id(task.get_process_group_id());
    child.set_controlling_tty(task.get_controlling_tty().as_ref().map(Arc::downgrade));
    child.set_nice(task.sched_nice.load(Ordering::Relaxed));
    child.set_core_preference(task.core_preference());
    child.set_scheduler_affinity_config(task.scheduler_affinity_kind(), task.cpu_affinity_mask());
    child.init();
    let mut next_tf = Trapframe::new();
    TransparentExecutor::execute_in_environment_with_abi(
        environment,
        &executable,
        &argv,
        &envp,
        &options.cwd,
        &options.handles,
        &task,
        &child,
        &mut next_tf,
        abi.as_deref(),
    )
    .map_err(|error| {
        crate::println!("Environment spawn: {}", error);
        "Environment spawn failed"
    })?;
    *child.get_trapframe() = next_tf;
    child.finish_exec_dispatch();
    let cpu = crate::sched::scheduler::select_cpu_for_task(&child);
    let id = crate::sched::scheduler::try_register_task(child)?;
    let child = crate::sched::scheduler::get_task_by_id(id).ok_or("child registration failed")?;
    let _ = task.adopt_registered_process_child(&child);
    let pid = child.get_namespace_id();
    crate::sched::scheduler::enqueue_task(id, cpu);
    Ok(pid)
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::PAGE_SIZE;
    use crate::library::std::usercopy::copy_to_user;
    use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};
    use alloc::boxed::Box;
    use scarlet_abi::RawEnvironmentExec;

    #[repr(C, align(4096))]
    struct UserPages([u8; 2 * PAGE_SIZE]);

    fn write_options(task: &Task, address: usize, options: &RawEnvironmentExec) {
        // Both native layouts have no padding: two u32s and five words. The
        // independent scarlet-abi host fixtures freeze their byte encodings.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                options as *const _ as *const u8,
                core::mem::size_of_val(options),
            )
        };
        copy_to_user(task, address, bytes).unwrap();
    }

    #[test_case]
    fn native_exec_options_decode_mapped_unaligned_and_cross_page_inputs() {
        // Keep the backing allocation alive until after the task's maps drop.
        let mut backing = Box::new(UserPages([0; 2 * PAGE_SIZE]));
        let task = crate::task::new_user_task("exec-options-abi".into(), 1);
        let base = 0x10000;
        let physical = crate::vm::virt_to_phys(backing.0.as_mut_ptr() as usize);
        task.vm_manager
            .add_memory_map(VirtualMemoryMap::new(
                crate::vm::vmem::PhysicalMemoryArea::new(
                    physical,
                    physical + 2 * PAGE_SIZE as u64 - 1,
                ),
                MemoryArea::new(base, base + 2 * PAGE_SIZE - 1),
                VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
                false,
                None,
            ))
            .unwrap();

        let arg = base + 0x20;
        let env = base + 0x40;
        let cwd = base + 0x60;
        let argv = base + PAGE_SIZE - 4;
        copy_to_user(&task, arg, b"program\0").unwrap();
        copy_to_user(&task, env, b"A=B\0").unwrap();
        copy_to_user(&task, cwd, b"/child\0").unwrap();
        copy_to_user(&task, argv, &arg.to_ne_bytes()).unwrap();
        copy_to_user(
            &task,
            argv + core::mem::size_of::<usize>(),
            &0usize.to_ne_bytes(),
        )
        .unwrap();
        copy_to_user(&task, base + 0x100, &env.to_ne_bytes()).unwrap();
        copy_to_user(
            &task,
            base + 0x180,
            &[4u32.to_ne_bytes(), 7u32.to_ne_bytes()].concat(),
        )
        .unwrap();

        let mut record = RawEnvironmentExec {
            size: core::mem::size_of::<RawEnvironmentExec>() as u32,
            flags: 0,
            argv,
            envp: base + 0x100,
            cwd,
            handles: base + 0x180,
            handle_count: 1,
        };
        let address = base + 0x201;
        write_options(&task, address, &record);
        let decoded = exec_options(&task, address).unwrap();
        assert_eq!(decoded.argv, ["program"]);
        assert_eq!(decoded.envp, ["A=B"]);
        assert_eq!(decoded.cwd, "/child");
        assert_eq!(decoded.handles.len(), 1);
        assert_eq!(
            (decoded.handles[0].source, decoded.handles[0].target),
            (4, 7)
        );
        // The legacy exec string helper must use the same checked word decoder.
        assert_eq!(
            crate::library::std::string::parse_string_array_from_userspace(&task, argv, 256, 4096)
                .unwrap(),
            ["program"]
        );

        record.flags = 1;
        write_options(&task, address, &record);
        assert_eq!(
            exec_options(&task, address).err(),
            Some("unsupported exec options")
        );
        record.flags = 0;
        record.handle_count = HandleTable::MAX_HANDLES + 1;
        write_options(&task, address, &record);
        assert_eq!(exec_options(&task, address).err(), Some("too many handles"));

        // A null-terminated empty record now straddles the page boundary.
        record.argv = 0;
        record.envp = 0;
        record.cwd = 0;
        record.handles = 0;
        record.handle_count = 0;
        let crossing = base + PAGE_SIZE - 13;
        write_options(&task, crossing, &record);
        let decoded = exec_options(&task, crossing).unwrap();
        assert!(decoded.argv.is_empty() && decoded.envp.is_empty() && decoded.handles.is_empty());
        assert_eq!(decoded.cwd, "/");
        assert_eq!(
            exec_options(&task, base + 2 * PAGE_SIZE - 1).err(),
            Some("invalid options")
        );
    }
}
