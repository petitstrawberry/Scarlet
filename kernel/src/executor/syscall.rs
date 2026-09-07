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
    for index in 0..=256 {
        let mut bytes = [0u8; 8];
        let slot = address.checked_add(index * 8).ok_or("invalid array")?;
        slot.checked_add(bytes.len()).ok_or("invalid array")?;
        copy_from_user(task, slot, &mut bytes).map_err(|_| "invalid array")?;
        let ptr = usize::from_ne_bytes(bytes);
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
    // RawEnvironmentExec: size:u32, flags:u32, then five native pointer-sized
    // words (argv, envp, cwd, handles, handle_count). Both targets are 64-bit.
    let mut bytes = [0u8; 48];
    address.checked_add(bytes.len()).ok_or("invalid options")?;
    copy_from_user(task, address, &mut bytes).map_err(|_| "invalid options")?;
    if u32::from_ne_bytes(bytes[0..4].try_into().unwrap()) != 48 || bytes[4..8] != [0; 4] {
        return Err("unsupported exec options");
    }
    let word = |n: usize| usize::from_ne_bytes(bytes[n..n + 8].try_into().unwrap());
    let handle_address = word(32);
    let count = word(40);
    if count > HandleTable::MAX_HANDLES {
        return Err("too many handles");
    }
    let mut handles = Vec::new();
    for index in 0..count {
        let mut entry = [0u8; 8];
        let address = handle_address
            .checked_add(index * 8)
            .ok_or("invalid handle map")?;
        address.checked_add(8).ok_or("invalid handle map")?;
        copy_from_user(task, address, &mut entry).map_err(|_| "invalid handle map")?;
        handles.push(HandleMapping {
            source: u32::from_ne_bytes(entry[..4].try_into().unwrap()),
            target: u32::from_ne_bytes(entry[4..].try_into().unwrap()),
        });
    }
    Ok(ExecOptions {
        argv: string_array(task, word(8))?,
        envp: string_array(task, word(16))?,
        cwd: if word(24) == 0 {
            "/".to_string()
        } else {
            text_arg(task, word(24))?
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
