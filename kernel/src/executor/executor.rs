//! ABI detection and transactional process-image replacement.
//!
//! Loaders operate on an unpublished task image. Until commit, the caller's
//! memory, descriptors, ABI, filesystem context and Environment are unchanged.

use super::environment::Environment;
use crate::{
    arch::Trapframe,
    fs::{VfsManager, vfs_v2::PathResolutionOptions},
    object::KernelObject,
    task::Task,
};
use alloc::{
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::{fmt, sync::atomic::Ordering};

// Linux's hard ceiling is 3/4 of _STK_LIM (8 MiB). Its currently reported
// stack rlimit exceeds that ceiling. Include pointer slots in the aggregate
// budget, and retain Linux's independent 32-page limit for each string.
pub(crate) const MAX_EXEC_STRING_BYTES: usize = 6 * 1024 * 1024;
pub(crate) const MAX_EXEC_STRING_LENGTH: usize = 32 * crate::environment::PAGE_SIZE;

pub(crate) fn exec_string_storage_bytes(length: usize) -> Option<usize> {
    if length >= MAX_EXEC_STRING_LENGTH {
        return None;
    }
    length.checked_add(1 + core::mem::size_of::<usize>())
}

pub(crate) fn exec_argument_bytes(argv: &[&str], envp: &[&str]) -> Option<usize> {
    argv.iter()
        .chain(envp)
        .try_fold(0usize, |bytes, string| {
            bytes.checked_add(exec_string_storage_bytes(string.len())?)
        })
        .filter(|bytes| *bytes <= MAX_EXEC_STRING_BYTES)
}

#[derive(Debug, Clone)]
pub enum ExecutorError {
    UnknownBinaryFormat,
    UnsupportedAbi(String),
    AbiUnavailableInEnvironment(String),
    OpenFailed {
        path: String,
        error: crate::fs::FileSystemError,
    },
    ExecutionFailed(String),
}

impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBinaryFormat => write!(f, "Unknown binary format"),
            Self::UnsupportedAbi(abi) => write!(f, "Unsupported ABI: {abi}"),
            Self::AbiUnavailableInEnvironment(abi) => {
                write!(f, "ABI unavailable in environment: {abi}")
            }
            Self::OpenFailed { path, error } => {
                write!(f, "Failed to open executable {path}: {}", error.message)
            }
            Self::ExecutionFailed(msg) => write!(f, "Execution failed: {msg}"),
        }
    }
}

pub type ExecutorResult<T> = Result<T, ExecutorError>;

fn failure(message: &str) -> ExecutorError {
    ExecutorError::ExecutionFailed(message.to_string())
}

/// Find a path inside a registered view by walking the VFS entries shared with
/// the caller's view. The closest registered root wins when views are nested.
fn path_in_environment_view(
    vfs: &VfsManager,
    path: &str,
    environment: &Environment,
    namespace: &crate::fs::vfs_v2::namespace::MountNamespace,
) -> Option<(String, Arc<crate::fs::vfs_v2::manager::VfsView>, String)> {
    let (entry, _) = vfs
        .resolve_path_with_options(path, &PathResolutionOptions::no_follow())
        .ok()?;
    let mut best = None;
    for (abi, view) in environment.roots() {
        let view = namespace.resolve(&view);
        let root = view.mount_tree.root_mount.read().root.clone();
        let mut current = entry.clone();
        let mut components = Vec::new();
        loop {
            if Arc::ptr_eq(&current, &root) {
                let depth = components.len();
                components.reverse();
                let relative = format!("/{}", components.join("/"));
                if best
                    .as_ref()
                    .is_none_or(|(best_depth, _, _, _)| depth < *best_depth)
                {
                    best = Some((depth, abi, view, relative));
                }
                break;
            }
            let (name, parent) = current.location();
            let Some(parent) = parent else { break };
            components.push(name);
            current = parent;
        }
    }
    best.map(|(_, abi, view, relative)| (abi, view, relative))
}

/// Explicit Environment transitions inherit only these source → target handles.
pub struct HandleMapping {
    pub source: u32,
    pub target: u32,
}

pub struct TransparentExecutor;

impl TransparentExecutor {
    pub fn execute_binary(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        task: &Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        Self::execute_path(path, argv, envp, None, task, trapframe, 0)
    }

    pub fn execute_with_abi(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        abi_name: &str,
        task: &Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        Self::execute_path(path, argv, envp, Some(abi_name), task, trapframe, 0)
    }

    fn execute_path(
        path: &str,
        argv: &[&str],
        envp: &[&str],
        explicit_abi: Option<&str>,
        task: &Task,
        trapframe: &mut Trapframe,
        depth: usize,
    ) -> ExecutorResult<()> {
        if depth >= 4 {
            return Err(failure("runtime delegation loop"));
        }
        let vfs = task
            .get_vfs()
            .ok_or_else(|| failure("missing filesystem context"))?;
        // Resolve the final entry without following it, then choose its
        // registered Environment view. Absolute links are followed in that
        // view, independent of where its tree is exposed to the caller.
        let environment = task.execution_environment.read().clone();
        let mount_namespace = task.mount_namespace.read().clone();
        let view_path = environment
            .as_ref()
            .and_then(|env| path_in_environment_view(&vfs, path, env, &mount_namespace));
        let file = if let Some((_, view, relative)) = &view_path {
            VfsManager::from_view(view.clone()).open(relative, 0)
        } else {
            vfs.open(path, 0)
        }
        .map_err(|error| ExecutorError::OpenFailed {
            path: path.to_string(),
            error,
        })?;
        if let Some(file_obj) = file.as_file() {
            if !file_obj.allows_execution() { return Err(failure("execution disabled on mount")); }
            let mut header = [0u8; 256];
            if let Ok(n) = file_obj.read_at(0, &mut header) {
                if n >= 2 && &header[..2] == b"#!" {
                    let end = header[..n]
                        .iter()
                        .position(|&byte| byte == b'\n')
                        .unwrap_or(n);
                    let line = core::str::from_utf8(&header[2..end])
                        .map_err(|_| failure("invalid script interpreter"))?;
                    let mut words = line.split_whitespace();
                    let interpreter = words
                        .next()
                        .filter(|name| name.starts_with('/'))
                        .ok_or_else(|| failure("invalid script interpreter"))?;
                    let script_path = view_path
                        .as_ref()
                        .map(|(_, _, relative)| relative.clone())
                        .unwrap_or_else(|| path.to_string());
                    let interpreter_path = interpreter.to_string();
                    let mut args = alloc::vec![interpreter_path.as_str()];
                    if let Some(argument) = words.next() {
                        args.push(argument);
                    }
                    args.push(script_path.as_str());
                    args.extend(argv.iter().skip(1).copied());
                    if let Some((_, view, _)) = &view_path {
                        let environment = environment
                            .clone()
                            .ok_or_else(|| failure("process has no Environment"))?;
                        let interpreter_file = VfsManager::from_view(view.clone())
                            .open(interpreter, 0)
                            .map_err(|error| ExecutorError::OpenFailed {
                                path: interpreter.to_string(),
                                error,
                            })?;
                        let name = Self::detect_abi(&interpreter_file, interpreter)?;
                        return Self::replace_image(
                            &interpreter_file,
                            interpreter,
                            &args,
                            envp,
                            &name,
                            Some(environment),
                            None,
                            None,
                            task,
                            task,
                            trapframe,
                        );
                    }
                    return Self::execute_path(
                        &interpreter_path,
                        &args,
                        envp,
                        explicit_abi,
                        task,
                        trapframe,
                        depth + 1,
                    );
                }
            }
        }
        let name = match explicit_abi {
            Some(name) => name.to_string(),
            None => Self::detect_abi(&file, path)?,
        };
        let abi = crate::abi::AbiRegistry::instantiate(&name)
            .ok_or_else(|| ExecutorError::UnsupportedAbi(name.clone()))?;
        if let Some(runtime) = abi.get_runtime_config(&file, path) {
            let mut args = alloc::vec![runtime.runtime_path.as_str()];
            args.extend(runtime.runtime_args.iter().map(String::as_str));
            args.push(path);
            args.extend(argv.iter().skip(1).copied());
            return Self::execute_path(
                &runtime.runtime_path,
                &args,
                envp,
                runtime.runtime_abi.as_deref(),
                task,
                trapframe,
                depth + 1,
            );
        }
        // Keep the executable identity valid after switching to its ABI view.
        // Linux programs use /proc/self/exe to locate helpers for posix_spawn.
        let view_identity = view_path
            .as_ref()
            .filter(|(view_name, _, _)| *view_name == name)
            .map(|(_, _, relative)| relative.clone());
        let identity = view_identity.as_deref().unwrap_or(path);
        let mut view_argv = argv.to_vec();
        if let Some(first) = view_argv.first_mut()
            && *first == path
        {
            *first = identity;
        }
        Self::replace_image(
            &file,
            identity,
            &view_argv,
            envp,
            &name,
            environment,
            None,
            None,
            task,
            task,
            trapframe,
        )
    }

    /// Execute an already-open image in a sealed Environment. No ambient handles
    /// cross this boundary; even standard streams must be explicitly mapped.
    pub fn execute_in_environment(
        environment: Arc<Environment>,
        file: &KernelObject,
        argv: &[&str],
        envp: &[&str],
        cwd: &str,
        handles: &[HandleMapping],
        source: &Task,
        target: &Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        Self::execute_in_environment_with_abi(
            environment,
            file,
            argv,
            envp,
            cwd,
            handles,
            source,
            target,
            trapframe,
            None,
        )
    }

    /// Optional explicit ABI selection for formats without an unambiguous ABI
    /// marker (for example xv6 ELF). It never selects another Environment.
    pub fn execute_in_environment_with_abi(
        environment: Arc<Environment>,
        file: &KernelObject,
        argv: &[&str],
        envp: &[&str],
        cwd: &str,
        handles: &[HandleMapping],
        source: &Task,
        target: &Task,
        trapframe: &mut Trapframe,
        explicit_abi: Option<&str>,
    ) -> ExecutorResult<()> {
        let identity = argv.first().copied().unwrap_or("");
        let name = match explicit_abi {
            Some(name) => name.to_string(),
            None => Self::detect_abi(file, identity)?,
        };
        Self::replace_image(
            file,
            identity,
            argv,
            envp,
            &name,
            Some(environment),
            Some(cwd),
            Some(handles),
            source,
            target,
            trapframe,
        )
    }

    fn detect_abi(file: &KernelObject, path: &str) -> ExecutorResult<String> {
        crate::abi::AbiRegistry::detect_best_abi(file, path)
            .map(|(name, _)| name)
            .ok_or(ExecutorError::UnknownBinaryFormat)
    }

    fn replace_image(
        file: &KernelObject,
        identity: &str,
        argv: &[&str],
        envp: &[&str],
        abi_name: &str,
        environment: Option<Arc<Environment>>,
        cwd: Option<&str>,
        handles: Option<&[HandleMapping]>,
        source: &Task,
        task: &Task,
        trapframe: &mut Trapframe,
    ) -> ExecutorResult<()> {
        if file.as_file().is_some_and(|f| !f.allows_execution()) {
            return Err(failure("execution disabled on mount"));
        }
        // Replacing a shared address space, shared descriptor table, or a live
        // thread group needs coordinated thread retirement, not partial exec.
        if !task.can_replace_exec_image() {
            return Err(failure(
                "exec requires an exclusive single-threaded process",
            ));
        }
        if exec_argument_bytes(argv, envp).is_none() {
            return Err(failure("argument list too large"));
        }
        let explicit_transition = handles.is_some();
        let mount_namespace = if explicit_transition {
            source.mount_namespace.read().clone()
        } else {
            task.mount_namespace.read().clone()
        };
        let current_vfs = task.get_vfs();
        let current_abi = task.with_default_abi(|abi| abi.get_name());
        let vfs = match &environment {
            Some(env) => {
                if !env.is_sealed() {
                    return Err(failure("Environment is not sealed"));
                }
                let view = env.root(abi_name).map_err(|_| {
                    ExecutorError::AbiUnavailableInEnvironment(abi_name.to_string())
                })?;
                let view = mount_namespace.resolve(&view);
                if !explicit_transition && current_abi == abi_name {
                    let fs = current_vfs.ok_or_else(|| failure("missing filesystem context"))?;
                    if !Arc::ptr_eq(&fs.view(), &view) {
                        return Err(failure("inconsistent Environment view"));
                    }
                    VfsManager::clone_with_shared_mount_namespace(&fs)
                } else {
                    VfsManager::from_view(view)
                }
            }
            None if !explicit_transition && task.bootstrap_environment.load(Ordering::Acquire) => {
                current_vfs.ok_or_else(|| failure("missing bootstrap filesystem"))?
            }
            None => return Err(failure("process has no Environment")),
        };
        if let Some(cwd) = cwd {
            if !cwd.starts_with('/') {
                return Err(failure("Environment cwd must be absolute"));
            }
            vfs.set_cwd_by_path(cwd)
                .map_err(|_| failure("invalid target working directory"))?;
        }

        let pending_transition = source.pending_exec_transition.read().clone();
        let mut image = task.new_exec_image();
        image.vm_manager.configure_layout(source.vm_manager.layout_policy()).map_err(failure)?;
        if explicit_transition {
            *image.security_subject.write() = source.security_subject.read().clone();
            *image.credentials.write() = source.credentials.read().clone();
            *image.execution_restrictions.write() = source.execution_restrictions.read().clone();
        }
        image.set_vfs(vfs);
        *image.mount_namespace.write() = mount_namespace;
        *image.execution_environment.write() = environment;
        if let Some(handles) = handles {
            for mapping in handles {
                let (object, metadata) = source
                    .handle_table
                    .clone_for_dup(mapping.source)
                    .ok_or_else(|| failure("invalid source handle"))?;
                // Mapping explicitly retains it for this transition. Preserve
                // CLOEXEC so delegated management handles do not leak through
                // subsequent ordinary execs.
                image
                    .handle_table
                    .insert_exec_handle(mapping.target, object, metadata)
                    .map_err(failure)?;
            }
        } else {
            image.handle_table = task.handle_table.deep_clone();
            image.handle_table.remove_close_on_exec();
        }
        let mut abi = crate::abi::AbiRegistry::instantiate(abi_name)
            .ok_or_else(|| ExecutorError::UnsupportedAbi(abi_name.to_string()))?;
        if explicit_transition {
            abi.prepare_exec_handles(None, &image).map_err(failure)?;
        } else {
            task.with_default_abi(|old| abi.prepare_exec_handles(Some(old), &image))
                .map_err(failure)?;
        }
        let no_file_grants = file.as_file().is_some_and(|f| !f.allows_execution_privilege_gain());
        let inherited_credentials = no_file_grants.then(|| image.credentials.read().clone());
        let inherited_subject = image.security_subject.read().clone();
        let resource_label = inherited_subject.as_ref().and_then(|subject| file.as_file()
            .and_then(|file| file.as_any().downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>())
            .and_then(|file| subject.policy().resource_label(file.get_vfs_entry().node().as_ref())));
        let mut next_trapframe = trapframe.clone();
        abi.execute_binary(file, argv, envp, &image, &mut next_trapframe)
            .map_err(failure)?;
        if let Some(inherited) = inherited_credentials {
            let subject_unchanged = match (&inherited_subject, &*image.security_subject.read()) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_identity(b),
                _ => false,
            };
            if *image.credentials.read() != inherited || !subject_unchanged {
                return Err(failure("file-derived privilege changes disabled on mount"));
            }
        }
        if let Some(pending) = pending_transition {
            let source_subject = source.security_subject.read().clone().ok_or_else(|| failure("exec transition source lost"))?;
            let prepared_subject = image.security_subject.read().clone().ok_or_else(|| failure("exec transition source lost"))?;
            if !source_subject.same_identity(&prepared_subject) { return Err(failure("exec transition source changed")); }
            let target = pending.resolve_for_resource(&source_subject, &image.credentials.read(), resource_label, !no_file_grants).map_err(failure)?;
            *image.security_subject.write() = Some(target);
        } else if let (Some(subject), Some(resource)) = (&inherited_subject, resource_label) {
            if subject.policy().has_exec_resource(subject.label(), resource) {
                if !image.security_subject.read().as_ref().is_some_and(|prepared| prepared.same_identity(subject)) {
                    return Err(failure("exec resource source changed"));
                }
                let target = subject.resource_exec(resource, &image.credentials.read()).map_err(failure)?;
                if no_file_grants && !target.same_identity(subject) { return Err(failure("file-derived label change disabled on mount")); }
                *image.security_subject.write() = Some(target);
            }
        }
        image.set_executable_path(identity);

        // All fallible operations are complete. Locks here only exchange owned
        // state; no filesystem I/O, loader callbacks, or allocation occurs.
        let commit_guard = crate::sync::IrqGuard::new();
        task.vm_manager.exchange_exec_image(&image.vm_manager);
        core::mem::swap(&mut *task.security_subject.write(), &mut *image.security_subject.write());
        *task.pending_exec_transition.write() = None;
        *task.creation_label.write() = None;
        core::mem::swap(&mut *task.credentials.write(), &mut *image.credentials.write());
        core::mem::swap(&mut *task.execution_restrictions.write(), &mut *image.execution_restrictions.write());
        task.handle_table.exchange_exec_table(&image.handle_table);
        core::mem::swap(&mut *task.name.write(), &mut *image.name.write());
        core::mem::swap(&mut *task.vcpu.lock(), &mut *image.vcpu.lock());
        core::mem::swap(&mut *task.vfs.write(), &mut *image.vfs.write());
        core::mem::swap(
            &mut *task.mount_namespace.write(),
            &mut *image.mount_namespace.write(),
        );
        core::mem::swap(
            &mut *task.execution_environment.write(),
            &mut *image.execution_environment.write(),
        );
        task.text_size
            .store(image.text_size.load(Ordering::Relaxed), Ordering::Relaxed);
        task.stack_size
            .store(image.stack_size.load(Ordering::Relaxed), Ordering::Relaxed);
        task.exchange_executable_path(&image);
        task.set_linux_clear_child_tid(None);
        abi.on_exec_commit(task);
        task.install_exec_abi(abi);
        if explicit_transition {
            task.bootstrap_environment.store(false, Ordering::Release);
        }
        *trapframe = next_trapframe;
        // Exec does not necessarily pass through the scheduler before returning
        // to userspace. Publish the new ASID to the current CPU's trampoline
        // before the old image (and its page tables) is retired. Preparing a
        // spawned child must never change the caller's return address space.
        if crate::task::mytask().is_some_and(|current| core::ptr::eq(&*current, task)) {
            crate::arch::get_cpu().set_next_address_space(task.vm_manager.get_asid());
        }
        drop(commit_guard);
        // The image now owns the retired memory and handles. Drop outside locks.
        drop(image);
        Ok(())
    }
}
