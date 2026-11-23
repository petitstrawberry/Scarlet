//! Process-local native dynamic-loader state, driven by the signed guest veneers.
use super::{GuestRun, LocalImage};
use crate::{
    arch::Trapframe,
    hosted::signed_exec::resolve_library,
    library::std::usercopy::copy_from_user,
    sched::scheduler,
    task::{Task, TaskState},
};
use alloc::{
    collections::{BTreeMap, VecDeque},
    string::String,
    vec::Vec,
};
use core::sync::atomic::Ordering;
use scarlet_hosted_runtime::{
    in_process::{LinkError, LinkOperation},
    run_gate::TaskId,
    signed_image::ObjectId,
};

const RTLD_NOW_GLOBAL: usize = 2 | 0x100;

#[derive(Clone)]
struct Initialization {
    object: ObjectId,
    constructors: VecDeque<usize>,
}

#[derive(Clone, Default)]
pub(super) struct LinkState {
    handles: BTreeMap<usize, Option<ObjectId>>,
    next_handle: usize,
    owner: Option<TaskId>,
    initializers: Vec<Initialization>,
    failed: bool,
    errors: BTreeMap<(usize, u64), LinkError>,
}

impl LinkState {
    pub(super) fn fork(&self, task: &Task) -> Self {
        let mut child = self.clone();
        child.errors.clear();
        child.owner = None;
        // The initializing thread can resume its copied constructor stack.
        // A different thread's child has no continuation for that work.
        if !self.initializers.is_empty()
            && self.owner.map_or_else(
                || task.get_thread_group_id() != task.get_id(),
                |owner| owner.id != task.get_id(),
            )
        {
            child.fail_initialization();
        }
        child
    }

    fn fail_initialization(&mut self) {
        self.failed = true;
        self.owner = None;
        self.initializers.clear();
    }

    // A new fork child starts with one task. Bind the copied continuation
    // before that task can run guest code or create siblings.
    pub(super) fn bind_fork_owner(&mut self, thread: TaskId) {
        if !self.initializers.is_empty() && self.owner.is_none() {
            self.owner = Some(thread);
        }
    }

    fn handle(&mut self, object: Option<ObjectId>) -> Result<usize, LinkError> {
        let handle = self.next_handle.checked_add(1).ok_or(LinkError::Handle)?;
        self.next_handle = handle;
        self.handles.insert(handle, object);
        Ok(handle)
    }

    fn scope(&self, handle: usize) -> Result<Option<ObjectId>, LinkError> {
        if handle == 0 {
            return Ok(None);
        }
        self.handles.get(&handle).copied().ok_or(LinkError::Handle)
    }

    // The guest veneer runs one constructor per continuation. Nested dlopen
    // has its own veneer frame and must finish before its caller advances.
    fn advance(&mut self) -> Result<(usize, usize), LinkError> {
        let pending = self.initializers.last_mut().ok_or(LinkError::Open)?;
        if let Some(pc) = pending.constructors.pop_front() {
            return Ok((0, pc));
        }
        let object = pending.object;
        self.initializers.pop();
        if self.initializers.is_empty() {
            self.owner = None;
        }
        self.handle(Some(object)).map(|handle| (handle, 0))
    }
}

pub(super) fn dispatch(
    image: &LocalImage,
    task: &Task,
    thread: TaskId,
    tf: &mut Trapframe,
    op: LinkOperation,
) -> GuestRun {
    let key = (thread.id, thread.generation);
    if matches!(op, LinkOperation::Error) {
        tf.regs.reg[0] = image
            .link
            .lock()
            .unwrap()
            .errors
            .remove(&key)
            .map_or(0, LinkError::guest_address);
        tf.regs.reg[1] = 0;
        return GuestRun::Resume;
    }

    // A page-in may yield. Copy the symbol before checking whether another
    // thread owns the loader, then obtain the address-space scope.
    let symbol_name = match op {
        LinkOperation::Symbol => guest_string(task, tf.regs.reg[1]),
        _ => None,
    };
    let access = {
        let mut state = image.link.lock().unwrap();
        match state.owner {
            Some(owner) if owner != thread && owner_alive(owner, task) => None,
            Some(owner) if owner != thread => {
                if state.initializers.is_empty() {
                    // A resolver that exited before publication modified no live object.
                    state.owner = None;
                } else {
                    state.fail_initialization();
                }
                Some(state.failed)
            }
            _ => Some(state.failed),
        }
    };
    let Some(failed) = access else {
        return GuestRun::RetryLoader;
    };
    let first = tf.regs.reg[0];
    let second = tf.regs.reg[1];
    let result = if failed {
        Err(LinkError::Incomplete)
    } else {
        match op {
            LinkOperation::Open => open(image, task, thread, first, second),
            LinkOperation::Continue => image.link.lock().unwrap().advance(),
            LinkOperation::Symbol => symbol_name
                .ok_or(LinkError::Symbol)
                .and_then(|name| symbol(image, first, &name, tf.tpidr_el0 as usize)),
            LinkOperation::Close => image
                .link
                .lock()
                .unwrap()
                .handles
                .remove(&first)
                .map(|_| (0, 0))
                .ok_or(LinkError::Handle),
            LinkOperation::Error => unreachable!(),
        }
    };
    let (first, second) = match result {
        Ok(value) => value,
        Err(error) => {
            image.link.lock().unwrap().errors.insert(key, error);
            (failure_return(op), 0)
        }
    };
    tf.regs.reg[0] = first;
    tf.regs.reg[1] = second;
    GuestRun::Resume
}

pub(super) fn failure_return(op: LinkOperation) -> usize {
    if matches!(op, LinkOperation::Close) {
        usize::MAX
    } else {
        0
    }
}

fn owner_alive(owner: TaskId, caller: &Task) -> bool {
    let pool = scheduler::get_task_pool();
    pool.task_generation(owner.id)
        .map(|generation| generation as u64)
        == Some(owner.generation)
        && scheduler::get_task_by_id(owner.id).is_some_and(|task| {
            caller.vm_manager.shares_address_space(&task.vm_manager)
                && !matches!(task.get_state(), TaskState::Zombie | TaskState::Terminated)
        })
}

fn symbol(
    image: &LocalImage,
    handle: usize,
    name: &str,
    tls: usize,
) -> Result<(usize, usize), LinkError> {
    let scope = image.link.lock().unwrap().scope(handle)?;
    let text = image.text.lock().unwrap().clone();
    text.lookup_symbol(scope, name, tls)
        .map_err(|_| LinkError::Symbol)?
        .map(|address| (address, 0))
        .ok_or(LinkError::Symbol)
}

fn open(
    image: &LocalImage,
    task: &Task,
    thread: TaskId,
    path: usize,
    flags: usize,
) -> Result<(usize, usize), LinkError> {
    if flags != RTLD_NOW_GLOBAL {
        return Err(LinkError::Flags);
    }
    if path == 0 {
        return image
            .link
            .lock()
            .unwrap()
            .handle(None)
            .map(|handle| (handle, 0));
    }
    image.link.lock().unwrap().owner = Some(thread);
    let result = load(image, task, path);
    if result.is_err() {
        let mut state = image.link.lock().unwrap();
        if state.initializers.is_empty() {
            state.owner = None;
        }
    }
    result
}

fn load(image: &LocalImage, task: &Task, path: usize) -> Result<(usize, usize), LinkError> {
    let path = guest_string(task, path).ok_or(LinkError::Open)?;
    let vfs = task.get_vfs();
    let text = image.text.lock().unwrap().clone();
    // Resolving VFS dependencies can sleep. Existing mappings stay live and
    // only this loader transaction can touch its private new leases.
    let library = text
        .load_library(
            &path,
            task.max_text_size,
            task.max_data_size,
            move |requester, name| resolve_library(vfs.as_deref(), requester, name),
        )
        .map_err(|error| {
            std::eprintln!("[hosted] dlopen {path}: {error}");
            LinkError::Open
        })?;

    // Use the same order as fork: mapping transaction, image, loader state.
    let _transaction = task.vm_manager.mapping_transaction();
    let mut text = image.text.lock().unwrap();
    let mut state = image.link.lock().unwrap();
    let data_bytes = library.text.committed_data_size();
    if library.text_bytes > task.max_text_size || data_bytes > task.max_data_size {
        return Err(LinkError::Open);
    }
    *text = library.text;
    task.text_size.store(library.text_bytes, Ordering::Relaxed);
    task.data_size.store(data_bytes, Ordering::Relaxed);
    state.initializers.push(Initialization {
        object: library.object,
        constructors: library.constructors.into(),
    });
    state.advance()
}

fn guest_string(task: &Task, address: usize) -> Option<String> {
    let mut bytes = Vec::new();
    for offset in 0..4096 {
        let mut byte = [0];
        copy_from_user(task, address.checked_add(offset)?, &mut byte).ok()?;
        if byte[0] == 0 {
            return String::from_utf8(bytes).ok();
        }
        bytes.push(byte[0]);
    }
    None
}
