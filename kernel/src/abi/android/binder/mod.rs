//! Initial 64-bit Binder adapter over native RPC objects. Supports bounded,
//! synchronous transactions, object references, descriptors and HIDL embedded
//! buffers and nested synchronous transactions over native RPC call chains.
//! One-way delivery is serialized until userspace frees each receive buffer.

use crate::{
    abi::linux::generic::{LinuxAbi, errno},
    arch::Trapframe,
    executor::environment::Environment,
    fs::{FileMetadata, FilePermission, FileType, SeekFrom},
    ipc::rpc::{RpcCall, RpcEndpoint, RpcError, RpcMessage, RpcRequest},
    library::std::usercopy::{copy_from_user, copy_to_user},
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, ReadySet,
            SelectWaitOutcome, Selectable, StreamError, StreamOps,
        },
    },
    sync::{
        IrqSpinLock, Once,
        waker::{WaitResult, Waker},
    },
    task::Task,
};
use alloc::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::any::Any;

mod arena;
mod buffers;
mod death;
pub(super) mod filesystem;
mod objects;
use arena::Arena;
use death::{BC_CLEAR_DEATH_NOTIFICATION, BC_DEAD_BINDER_DONE, BC_REQUEST_DEATH_NOTIFICATION};
use objects::{
    BC_ACQUIRE, BC_ACQUIRE_DONE, BC_DECREFS, BC_INCREFS, BC_INCREFS_DONE, BC_RELEASE, Objects,
};

const WRITE_READ: u32 = 0xc0306201;
const VERSION: u32 = 0xc0046209;
const SET_CONTEXT: u32 = 0x40046207;
const SET_CONTEXT_EXT: u32 = 0x4018620d;
const SET_MAX_THREADS: u32 = 0x40046205;
const THREAD_EXIT: u32 = 0x40046208;
const BC_TRANSACTION: u32 = 0x40406300;
const BC_REPLY: u32 = 0x40406301;
const BC_TRANSACTION_SG: u32 = 0x40486311;
const BC_REPLY_SG: u32 = 0x40486312;
const BC_FREE_BUFFER: u32 = 0x40086303;
const BC_REGISTER_LOOPER: u32 = 0x630b;
const BC_ENTER_LOOPER: u32 = 0x630c;
const BC_EXIT_LOOPER: u32 = 0x630d;
const BR_TRANSACTION: u32 = 0x80407202;
const BR_TRANSACTION_SEC_CTX: u32 = 0x80487202;
const BR_REPLY: u32 = 0x80407203;
const BR_DEAD_REPLY: u32 = 0x7205;
const BR_COMPLETE: u32 = 0x7206;
const BR_FAILED_REPLY: u32 = 0x7209;
const BR_NOOP: u32 = 0x720c;
const BR_SPAWN_LOOPER: u32 = 0x720d;
// Framework BinderProxy uses this userspace flag to request AppOps collection.
// Transport it unchanged; collection itself belongs to the Java recipient.
const FLAG_COLLECT_NOTED_APP_OPS: u32 = 0x2;
const MAX_THREADS: usize = 64;
const MAX_DATA: usize = crate::ipc::rpc::MAX_MESSAGE_SIZE - objects::HEADER;

fn u32_at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], p: usize) -> usize {
    u64::from_le_bytes(b[p..p + 8].try_into().unwrap()) as usize
}
fn put64(b: &mut [u8], p: usize, v: usize) {
    b[p..p + 8].copy_from_slice(&(v as u64).to_le_bytes());
}
fn read<const N: usize>(task: &Task, ptr: usize) -> Result<[u8; N], usize> {
    let mut b = [0; N];
    copy_from_user(task, ptr, &mut b).map_err(|_| errno::EFAULT)?;
    Ok(b)
}

fn enter_request_priority(request: &RpcRequest, task: &Task) -> Result<(), usize> {
    // Framework Binder clients publish a scheduling priority for many ordinary
    // calls. A fair-scheduled receiver has no fixed priority to raise, so
    // applying that request as a hard error makes stock Binder loops exit on
    // EAGAIN. Keep the donation contract for fixed-priority receivers.
    if task.fixed_priority.base() != 0 {
        request.enter_priority(task).map_err(|_| errno::EAGAIN)?;
    }
    Ok(())
}

fn write(task: &Task, ptr: usize, b: &[u8]) -> Result<(), usize> {
    copy_to_user(task, ptr, b).map_err(|_| errno::EFAULT)
}

// A service manager may inspect peer labels only inside its Shield policy.
// This is domain confinement; service permissions are checked by libselinux.
fn same_policy(a: Option<&crate::shield::Subject>, b: Option<&crate::shield::Subject>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a.policy(), b.policy()),
        _ => false,
    }
}
fn same_subject(a: Option<&crate::shield::Subject>, b: Option<&crate::shield::Subject>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_identity(b),
        _ => false,
    }
}

struct Context {
    endpoint: Weak<RpcEndpoint>,
    object: Arc<crate::ipc::rpc::RpcObject>,
    subject: Option<crate::shield::Subject>,
}
struct Domain {
    // Retain the environment so pointer identity cannot be reused while open.
    environment: Arc<Environment>,
    device: u8,
    context: IrqSpinLock<Option<Context>>,
    objects: objects::Registry,
}
type Domains = BTreeMap<(usize, u8), Weak<Domain>>;
static DOMAINS: Once<IrqSpinLock<Domains>> = Once::new();
fn domain(environment: Arc<Environment>, device: u8) -> Arc<Domain> {
    let key = (Arc::as_ptr(&environment) as usize, device);
    let mut domains = DOMAINS
        .call_once(|| IrqSpinLock::new(BTreeMap::new()))
        .lock();
    domains.retain(|_, domain| domain.strong_count() > 0);
    if let Some(domain) = domains.get(&key).and_then(Weak::upgrade) {
        return domain;
    }
    let domain = Arc::new(Domain {
        environment,
        device,
        context: IrqSpinLock::new(None),
        objects: IrqSpinLock::new(BTreeMap::new()),
    });
    domains.insert(key, Arc::downgrade(&domain));
    domain
}

#[derive(Default)]
struct Thread {
    looper: bool,
    registered: bool,
    waiting_for_process: bool,
    flush_generation: u64,
    returned_flush: u64,
    completed: VecDeque<Completed>,
    stack: Vec<Transaction>,
    pending_receive: Option<RpcRequest>,
    pending_delivery: Option<Delivery>,
}
// Retain a dequeued message while its receive allocation is unavailable.
// Native completion/request ownership must survive arena backpressure.
struct Delivery {
    message: Result<RpcMessage, RpcError>,
    incoming: Option<RpcRequest>,
    replying: bool,
}
impl Delivery {
    fn allocation_size(&self) -> usize {
        // The wire representation includes all data, offsets and HIDL buffers.
        // Leave room for the authenticated sender context on incoming calls.
        self.message.as_ref().map_or(8, |message| {
            message.bytes.len().saturating_add(if self.replying {
                0
            } else {
                crate::shield::MAX_LABEL_BYTES + 8
            })
        })
    }
}
struct Incoming {
    request: RpcRequest,
    accepts_fds: bool,
}
enum Transaction {
    Outgoing(RpcCall),
    Incoming(Incoming),
}
impl Thread {
    fn outgoing(&self) -> Option<&RpcCall> {
        match self.stack.last() {
            Some(Transaction::Outgoing(call)) => Some(call),
            _ => None,
        }
    }
    fn incoming(&self) -> Option<&Incoming> {
        match self.stack.last() {
            Some(Transaction::Incoming(request)) => Some(request),
            _ => None,
        }
    }
    fn can_receive(&self, endpoint: &RpcEndpoint) -> bool {
        if self.pending_receive.is_some() {
            true
        } else if let Some(call) = self.outgoing() {
            call.reentrant_ready(endpoint)
        } else {
            self.accepts_process_work() && endpoint.ready()
        }
    }
    fn receive(
        &mut self,
        endpoint: &RpcEndpoint,
    ) -> Result<Option<RpcRequest>, crate::ipc::rpc::RpcError> {
        if let Some(request) = self.pending_receive.take() {
            return Ok(Some(request));
        }
        if let Some(call) = self.outgoing() {
            call.receive_reentrant(endpoint)
        } else {
            endpoint.receive()
        }
    }

    /// Process-wide Binder work is eligible only when this thread has no
    /// transaction state or thread-local work still owed to userspace.
    ///
    /// Linux `binder_thread_read()` gives `thread->todo` priority over
    /// `proc->todo`. In particular, a one-way `BR_TRANSACTION_COMPLETE` is
    /// thread work and must be consumed before a process-level death notice;
    /// death callbacks can issue nested Binder calls, so returning one first
    /// can make an inner call consume the outer call's uncorrelated completion.
    fn accepts_process_work(&self) -> bool {
        self.looper
            && self.stack.is_empty()
            && self.completed.is_empty()
            && self.pending_receive.is_none()
            && self.pending_delivery.is_none()
    }
}
struct Completed {
    command: u32,
    acquisitions: Vec<Arc<objects::Node>>,
}
impl Completed {
    fn ready(&self) -> bool {
        self.acquisitions.iter().all(|node| node.acquired())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpawnRequest {
    Writing(usize),
    Requested,
}
struct State {
    closed: bool,
    max_threads: usize,
    spawn: Option<SpawnRequest>,
    threads: BTreeMap<usize, Thread>,
    arena: Option<Arc<Arena>>,
    node: (usize, usize),
    node_flags: u32,
}
impl State {
    fn can_spawn(&self, tid: usize) -> bool {
        !self.closed
            && self.spawn.is_none()
            && self.threads.get(&tid).is_some_and(|thread| thread.looper)
            && self
                .threads
                .values()
                .filter(|thread| thread.registered)
                .count()
                < self.max_threads
            && self.threads.len() < MAX_THREADS
            && !self
                .threads
                .iter()
                .any(|(&other, thread)| other != tid && thread.waiting_for_process)
    }

    fn thread(&self, tid: usize) -> Option<&Thread> {
        if self.closed {
            None
        } else {
            self.threads.get(&tid)
        }
    }

    fn thread_mut(&mut self, tid: usize) -> Result<&mut Thread, usize> {
        if self.closed {
            return Err(errno::EPIPE);
        }
        self.threads.get_mut(&tid).ok_or(errno::EPIPE)
    }
}
pub(crate) struct BinderFile {
    owner: usize,
    subject: Option<crate::shield::Subject>,
    domain: Arc<Domain>,
    endpoint: Arc<RpcEndpoint>,
    state: IrqSpinLock<State>,
    objects: IrqSpinLock<Objects>,
    send_gate: IrqSpinLock<()>,
    auto_fds: IrqSpinLock<BTreeMap<usize, Vec<crate::abi::linux::generic::DescriptorReceipt>>>,
}

pub(super) fn new_file(task: &Task, device: u8) -> Result<Arc<BinderFile>, usize> {
    if device > 2 {
        return Err(errno::EINVAL);
    }
    let environment = task
        .execution_environment
        .read()
        .clone()
        .ok_or(errno::ENODEV)?;
    Ok(Arc::new(BinderFile {
        owner: task.get_thread_group_id(),
        subject: task.security_subject.read().clone(),
        domain: domain(environment, device),
        endpoint: RpcEndpoint::new(),
        send_gate: IrqSpinLock::new(()),
        auto_fds: IrqSpinLock::new(BTreeMap::new()),
        objects: IrqSpinLock::new(Objects::default()),
        state: IrqSpinLock::new(State {
            closed: false,
            max_threads: 0,
            spawn: None,
            threads: BTreeMap::new(),
            arena: None,
            node: (0, 0),
            node_flags: 0,
        }),
    }))
}

pub(super) fn from_object(object: &KernelObject) -> Option<&BinderFile> {
    let mut file = object.as_file()?;
    loop {
        if let Some(binder) = file.as_any().downcast_ref::<BinderFile>() {
            return Some(binder);
        }
        file = file
            .as_any()
            .downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>()?
            .inner()
            .as_ref();
    }
}

impl BinderFile {
    fn check_owner(&self, task: &Task) -> Result<(), usize> {
        if task.get_thread_group_id() == self.owner
            && same_subject(self.subject.as_ref(), task.security_subject.read().as_ref())
            && task
                .execution_environment
                .read()
                .as_ref()
                .is_some_and(|environment| Arc::ptr_eq(environment, &self.domain.environment))
        {
            if self.state.lock().closed {
                Err(errno::EPIPE)
            } else {
                Ok(())
            }
        } else {
            Err(errno::EINVAL)
        }
    }
    fn ensure_thread(&self, tid: usize) -> Result<(), usize> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(errno::EPIPE);
        }
        if !state.threads.contains_key(&tid) {
            if state.threads.len() >= MAX_THREADS {
                return Err(errno::ENOMEM);
            }
            state.threads.insert(tid, Thread::default());
        }
        Ok(())
    }
    pub(crate) fn ioctl(
        &self,
        task: &Task,
        abi: &LinuxAbi,
        tf: &mut Trapframe,
        request: u32,
        arg: usize,
        nonblocking: bool,
    ) -> usize {
        let result = (|| {
            self.check_owner(task)?;
            if self.state.lock().closed {
                return Err(errno::EPIPE);
            }
            match request {
                VERSION => write(task, arg, &8i32.to_le_bytes()),
                SET_MAX_THREADS => {
                    let n = u32_at(&read::<4>(task, arg)?, 0);
                    if n as usize > MAX_THREADS {
                        Err(errno::EINVAL)
                    } else {
                        let mut state = self.state.lock();
                        if state.closed {
                            return Err(errno::EPIPE);
                        }
                        state.max_threads = n as usize;
                        drop(state);
                        self.endpoint.waker().wake_all();
                        Ok(())
                    }
                }
                SET_CONTEXT | SET_CONTEXT_EXT => {
                    let node = if request == SET_CONTEXT_EXT {
                        let object = read::<24>(task, arg)?;
                        // Context-manager scheduling floors remain unsupported.
                        // Security context requests use native sender metadata.
                        // This ioctl consumes flags, not a serialized Binder
                        // object. Stock ProcessState leaves the header zero.
                        if u32_at(&object, 4) & !0x1100 != 0 {
                            return Err(errno::EOPNOTSUPP);
                        }
                        (
                            (u64_at(&object, 8), u64_at(&object, 16)),
                            u32_at(&object, 4),
                        )
                    } else {
                        ((0, 0), 0)
                    };
                    let mut context = self.domain.context.lock();
                    if context
                        .as_ref()
                        .and_then(|c| c.endpoint.upgrade())
                        .is_some()
                    {
                        return Err(errno::EBUSY);
                    }
                    {
                        let mut state = self.state.lock();
                        if state.closed {
                            return Err(errno::EPIPE);
                        }
                        state.node = node.0;
                        state.node_flags = node.1;
                    }
                    *context = Some(Context {
                        endpoint: Arc::downgrade(&self.endpoint),
                        object: self.endpoint.create_object().map_err(|_| errno::ENOMEM)?,
                        subject: self.subject.clone(),
                    });
                    Ok(())
                }
                THREAD_EXIT => {
                    self.thread_exit(task.get_id());
                    Ok(())
                }
                WRITE_READ => self.write_read(task, abi, tf, arg, nonblocking),
                _ => Err(errno::ENOTTY),
            }
        })();
        result.map_or_else(errno::to_result, |_| 0)
    }
    fn command(&self, task: &Task, abi: &LinuxAbi, command: u32, data: &[u8]) -> Result<(), usize> {
        let command = match command {
            BC_TRANSACTION_SG => BC_TRANSACTION,
            BC_REPLY_SG => BC_REPLY,
            other => other,
        };
        let tid = task.get_id();
        if self.state.lock().closed {
            return Err(errno::EPIPE);
        }
        match command {
            BC_REQUEST_DEATH_NOTIFICATION => {
                self.objects
                    .lock()
                    .request_death(u32_at(data, 0), u64_at(data, 4), self)?;
            }
            BC_CLEAR_DEATH_NOTIFICATION | BC_DEAD_BINDER_DONE => {
                let target = self
                    .state
                    .lock()
                    .thread(tid)
                    .map_or(false, |thread| thread.looper)
                    .then_some(tid);
                let mut objects = self.objects.lock();
                if objects.is_closed() {
                    return Err(errno::EPIPE);
                }
                if command == BC_CLEAR_DEATH_NOTIFICATION {
                    objects
                        .deaths
                        .clear(u32_at(data, 0), u64_at(data, 4), target)?;
                } else {
                    objects.deaths.done(u64_at(data, 0), target)?;
                }
            }
            BC_ENTER_LOOPER | BC_REGISTER_LOOPER | BC_EXIT_LOOPER => {
                let mut state = self.state.lock();
                if command == BC_REGISTER_LOOPER {
                    if state.thread_mut(tid)?.looper || state.spawn != Some(SpawnRequest::Requested)
                    {
                        return Err(errno::EINVAL);
                    }
                    state.spawn = None;
                    let thread = state.thread_mut(tid)?;
                    thread.looper = true;
                    thread.registered = true;
                } else {
                    let thread = state.thread_mut(tid)?;
                    if command == BC_ENTER_LOOPER && thread.looper {
                        return Err(errno::EINVAL);
                    }
                    thread.looper = command == BC_ENTER_LOOPER;
                    thread.registered = false;
                    thread.waiting_for_process = false;
                }
                drop(state);
                self.endpoint.waker().wake_all();
            }
            BC_FREE_BUFFER => {
                let state = self.state.lock();
                if state.closed {
                    return Err(errno::EPIPE);
                }
                let arena = state.arena.clone().ok_or(errno::EINVAL)?;
                drop(state);
                self.release_buffer(&arena, u64_at(data, 0))?;
                // A process can have more outstanding synchronous Binder calls
                // than receive slots. Releasing one buffer must wake loopers
                // waiting to deliver the next request or reply.
                self.endpoint.waker().wake_all();
            }
            BC_INCREFS | BC_ACQUIRE | BC_RELEASE | BC_DECREFS | BC_INCREFS_DONE
            | BC_ACQUIRE_DONE => {
                self.objects.lock().reference_command(command, data)?;
            }
            BC_TRANSACTION | BC_REPLY => {
                // hwbinder's default INHERIT_RT flag is inert between Fair
                // tasks. Do not turn a native reservation into an unimplemented
                // inherited FIFO/RR transaction.
                if self.domain.device == 1 && task.deadline_enabled() {
                    return Err(errno::EOPNOTSUPP);
                }
                let flags = u32_at(data, 20);
                if flags
                    & !(FLAG_COLLECT_NOTED_APP_OPS
                        | 0x10
                        | 0x20
                        | if command == BC_REPLY { 8 } else { 1 })
                    != 0
                {
                    return Err(errno::EOPNOTSUPP);
                }
                // target.handle is a u32 member of an 8-byte union. Its
                // unused upper word is not part of the handle value.
                let oneway = command == BC_TRANSACTION && flags & 1 != 0;
                let number = u32_at(data, 0);
                let prepared = objects::prepare(task, abi, data)?;
                // Serialize reference publication through transport admission.
                // All fallible usercopy precedes this IRQ lock. A failed send
                // releases its leases before another exporter can adopt them.
                let _sending = self.send_gate.lock();
                if self.state.lock().closed {
                    return Err(errno::EPIPE);
                }
                let target = if command == BC_TRANSACTION && number != 0 {
                    Some(self.objects.lock().target(number as u32)?)
                } else {
                    None
                };
                let capability = target.as_ref().map(|lease| lease.node.object.clone());
                if task.deadline_enabled()
                    && target
                        .as_ref()
                        .is_some_and(|target| target.node.inherits_rt())
                {
                    return Err(errno::EOPNOTSUPP);
                }
                if !prepared.capabilities.is_empty() {
                    let accepts = if command == BC_TRANSACTION {
                        target
                            .as_ref()
                            .is_some_and(|target| target.node.accepts_fds())
                    } else {
                        self.state.lock().thread(tid).is_some_and(|thread| {
                            thread
                                .incoming()
                                .is_some_and(|incoming| incoming.accepts_fds)
                        })
                    };
                    if !accepts {
                        return Err(errno::EPERM);
                    }
                }
                if target.as_ref().is_some_and(|target| {
                    !same_policy(self.subject.as_ref(), target.node.subject.as_ref())
                }) {
                    return Err(errno::EACCES);
                }
                let (message, acquisitions) = self.encode(task, data, prepared, target)?;
                if command == BC_TRANSACTION {
                    let context = if number == 0 {
                        let context = self.domain.context.lock();
                        if context.as_ref().is_some_and(|c| {
                            !same_policy(self.subject.as_ref(), c.subject.as_ref())
                        }) {
                            return Err(errno::EACCES);
                        }
                        context.as_ref().and_then(|c| c.endpoint.upgrade())
                    } else {
                        None
                    };
                    let mut state = self.state.lock();
                    let thread = state.thread_mut(tid)?;
                    if !oneway {
                        if thread.outgoing().is_some() {
                            return Err(errno::EINVAL);
                        }
                        if thread.stack.len() >= crate::ipc::rpc::MAX_CALL_DEPTH {
                            return Err(errno::E2BIG);
                        }
                    }
                    if thread.completed.len() >= crate::ipc::rpc::MAX_IN_FLIGHT {
                        return Err(errno::EAGAIN);
                    }
                    if oneway {
                        let sent = if let Some(capability) = capability {
                            capability.send_oneway_from(task, message)
                        } else if let Some(context) = context {
                            context.send_oneway_from(task, message)
                        } else {
                            Err(crate::ipc::rpc::RpcError::PeerClosed)
                        };
                        match sent {
                            Ok(()) => thread.completed.push_back(Completed {
                                command: BR_COMPLETE,
                                acquisitions,
                            }),
                            Err(crate::ipc::rpc::RpcError::PeerClosed) => {
                                thread.completed.push_back(Completed {
                                    command: BR_DEAD_REPLY,
                                    acquisitions: Vec::new(),
                                })
                            }
                            // Binder one-way calls have no reply consumer to
                            // observe transport backpressure. Complete the
                            // command instead of aborting the stock looper;
                            // the application-facing notification is dropped.
                            Err(crate::ipc::rpc::RpcError::Busy) => {
                                thread.completed.push_back(Completed {
                                    command: BR_COMPLETE,
                                    acquisitions: Vec::new(),
                                })
                            }
                            Err(_) => return Err(errno::EAGAIN),
                        }
                        return Ok(());
                    }
                    let parent = thread
                        .incoming()
                        .and_then(|incoming| incoming.request.context());
                    let call = if let Some(capability) = capability {
                        capability.send_scoped_from(task, message, &self.endpoint, parent.as_ref())
                    } else if let Some(context) = context {
                        context.send_scoped_from(task, message, &self.endpoint, parent.as_ref())
                    } else {
                        Err(crate::ipc::rpc::RpcError::PeerClosed)
                    };
                    match call {
                        Ok(call) => {
                            call.notify_on_completion(&self.endpoint.waker());
                            thread.stack.push(Transaction::Outgoing(call));
                            thread.completed.push_back(Completed {
                                command: BR_COMPLETE,
                                acquisitions,
                            });
                        }
                        Err(crate::ipc::rpc::RpcError::PeerClosed) => {
                            thread.completed.push_back(Completed {
                                command: BR_DEAD_REPLY,
                                acquisitions: Vec::new(),
                            })
                        }
                        Err(crate::ipc::rpc::RpcError::TooLarge) => return Err(errno::E2BIG),
                        Err(crate::ipc::rpc::RpcError::Cancelled) => return Err(errno::ECANCELED),
                        Err(crate::ipc::rpc::RpcError::Busy) => {
                            thread.completed.push_back(Completed {
                                command: BR_FAILED_REPLY,
                                acquisitions: Vec::new(),
                            })
                        }
                        Err(_) => return Err(errno::EAGAIN),
                    }
                } else {
                    let request = {
                        let mut state = self.state.lock();
                        let thread = state.thread_mut(tid)?;
                        if thread.completed.len() >= crate::ipc::rpc::MAX_IN_FLIGHT {
                            return Err(errno::EAGAIN);
                        }
                        if thread.incoming().is_none() {
                            return Err(errno::EINVAL);
                        }
                        match thread.stack.pop().unwrap() {
                            Transaction::Incoming(incoming) => incoming.request,
                            _ => unreachable!(),
                        }
                    };
                    let _ = request.reply(message);
                    let mut state = self.state.lock();
                    state.thread_mut(tid)?.completed.push_back(Completed {
                        command: BR_COMPLETE,
                        acquisitions,
                    });
                }
            }
            _ => return Err(errno::EOPNOTSUPP),
        }
        Ok(())
    }
    pub(super) fn flush_on_close(&self) {
        // Linux binder_flush requests an ioctl return on every descriptor
        // close, independently of the file's final release. In particular,
        // app_process closes the driver while a Binder pool thread is waiting.
        {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            for thread in state.threads.values_mut() {
                thread.flush_generation = thread.flush_generation.wrapping_add(1);
            }
        }
        self.endpoint.waker().wake_all();
    }
    fn ready(&self, tid: usize) -> bool {
        let state = self.state.lock();
        if state.closed {
            return true;
        }
        let Some(thread) = state.threads.get(&tid) else {
            return false;
        };
        if thread.flush_generation != thread.returned_flush {
            return true;
        }
        // A looper can be blocked in an outgoing call while unrelated work is
        // queued for its process. Wake it to grow the pool; that work must not
        // be delivered as if it were a callback in the outstanding call chain.
        if state.can_spawn(tid) && self.endpoint.ready() {
            return true;
        }
        if thread
            .pending_delivery
            .as_ref()
            .and_then(|delivery| delivery.incoming.as_ref())
            .is_some_and(RpcRequest::cancelled)
        {
            return true;
        }
        let objects = self.objects.lock();
        let process_work = thread.accepts_process_work();
        if objects.deaths.ready(tid, process_work) {
            return true;
        }
        if objects.ready_for(tid, process_work) {
            return true;
        }
        if objects.pending_acquisition(tid) {
            return false;
        }
        if let Some(completed) = thread.completed.front() {
            return completed.ready();
        }
        let has_delivery = thread.pending_delivery.is_some()
            || thread.outgoing().is_some_and(RpcCall::ready)
            || thread.can_receive(&self.endpoint);
        !objects.pending_local_acquisitions()
            && has_delivery
            && state.arena.as_ref().is_none_or(|arena| {
                arena.has_space(
                    thread
                        .pending_delivery
                        .as_ref()
                        .map_or(8, Delivery::allocation_size),
                )
            })
    }
    fn waiter(&self, tid: usize) -> Arc<Waker> {
        // Incoming calls, object-reference work and outgoing completions all
        // notify this process source; readiness remains specific to each thread.
        let _ = tid;
        self.endpoint.waker()
    }
    fn thread_exit(&self, tid: usize) {
        let removed = {
            let mut state = self.state.lock();
            if state.spawn == Some(SpawnRequest::Writing(tid)) {
                state.spawn = None;
            }
            state.threads.remove(&tid)
        };
        let revoked = {
            let mut objects = self.objects.lock();
            objects.deaths.thread_exit(tid);
            objects.abandon_thread(tid)
        };
        // Native cancellation and lease destructors run outside Binder locks.
        for object in revoked {
            object.revoke();
        }
        drop(removed);
        self.endpoint.waker().wake_all();
    }
    fn release_buffer(&self, arena: &Arena, address: usize) -> Result<(), usize> {
        let mut objects = self.objects.lock();
        arena.free(address)?;
        objects.free_buffer(address);
        let request = objects.oneway.remove(&address);
        drop(objects);
        let descriptors = self.auto_fds.lock().remove(&address);
        drop(descriptors);
        // Release native serialization and budget only after userspace has
        // returned the receive buffer; other loopers can then dispatch it.
        drop(request);
        Ok(())
    }

    fn read_record(&self, task: &Task, output: usize, capacity: usize) -> Result<usize, usize> {
        self.read_record_for_abi(task, None, output, capacity)
    }
    fn read_record_for_abi(
        &self,
        task: &Task,
        abi: Option<&LinuxAbi>,
        output: usize,
        capacity: usize,
    ) -> Result<usize, usize> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(errno::EPIPE);
        }
        if self.domain.device == 1 && task.deadline_enabled() {
            return Err(errno::EOPNOTSUPP);
        }
        let tid = task.get_id();
        let thread = state.thread(tid).ok_or(errno::EPIPE)?;
        let cancel_receive = thread
            .pending_receive
            .as_ref()
            .is_some_and(RpcRequest::cancelled);
        let cancel_delivery = thread
            .pending_delivery
            .as_ref()
            .and_then(|delivery| delivery.incoming.as_ref())
            .is_some_and(RpcRequest::cancelled);
        if cancel_receive {
            let cancelled = state.thread_mut(tid)?.pending_receive.take();
            drop(state);
            drop(cancelled);
            return self.read_record_for_abi(task, abi, output, capacity);
        }
        if cancel_delivery {
            let cancelled = state.thread_mut(tid)?.pending_delivery.take();
            drop(state);
            drop(cancelled);
            return self.read_record_for_abi(task, abi, output, capacity);
        }
        let thread = state.thread(tid).ok_or(errno::EPIPE)?;
        if thread.flush_generation != thread.returned_flush {
            if capacity < 4 {
                return Err(errno::ENOSPC);
            }
            let generation = thread.flush_generation;
            drop(state);
            write(task, output, &BR_NOOP.to_le_bytes())?;
            // Usercopy may fault or race another close. A failed copy does not
            // consume the notification, and a newer generation stays pending.
            self.state.lock().thread_mut(tid)?.returned_flush = generation;
            return Ok(4);
        }
        if state.can_spawn(tid) && self.endpoint.ready() {
            if capacity < 4 {
                return Err(errno::ENOSPC);
            }
            // Reserve one request before dropping the lock for faultable
            // usercopy. A failed copy must not permanently consume pool space.
            state.spawn = Some(SpawnRequest::Writing(tid));
            drop(state);
            let copied = write(task, output, &BR_SPAWN_LOOPER.to_le_bytes());
            let mut state = self.state.lock();
            if state.spawn == Some(SpawnRequest::Writing(tid)) {
                state.spawn = copied.as_ref().ok().map(|_| SpawnRequest::Requested);
            }
            let closed = state.closed;
            drop(state);
            self.endpoint.waker().wake_all();
            copied?;
            if closed {
                return Err(errno::EPIPE);
            }
            return Ok(4);
        }
        let process_work = thread.accepts_process_work();
        let death = {
            let objects = self.objects.lock();
            if objects.deaths.ready(tid, process_work) {
                if capacity < 12 {
                    return Err(errno::ENOSPC);
                }
                objects.deaths.record(tid, process_work)
            } else {
                None
            }
        };
        if let Some(record) = death {
            drop(state);
            return record.write(task, output);
        }
        if self.objects.lock().ready_for(tid, process_work) {
            if capacity < 20 {
                return Err(errno::ENOSPC);
            }
            drop(state);
            let record = {
                // Do not expose acquisition work from an export until its
                // transport admission has committed. A failed send must drop
                // the corresponding lease before another thread observes it.
                let _sending = self.send_gate.lock();
                self.objects.lock().reference_record_for(tid, process_work)
            };
            return record.map_or(Ok(0), |record| record.write(task, output));
        }
        // Exporting code can drop its last local reference after completion.
        // Finish the driver's acquire handshakes before reporting completion.
        if self.objects.lock().pending_acquisition(tid) {
            return Ok(0);
        }
        if let Some(completed) = state
            .thread(tid)
            .and_then(|thread| thread.completed.front())
        {
            // Another thread may have begun this node's weak acquisition
            // before this sender exported it strongly. Both exporters retain
            // their userspace references until all needed holds are confirmed.
            if !completed.ready() {
                return Ok(0);
            }
            let command = completed.command;
            if capacity < 4 {
                return Err(errno::ENOSPC);
            }
            // Usercopy may resolve a lazy mapping; never hold an IRQ lock
            // across that operation. Only this task consumes its thread queue.
            drop(state);
            write(task, output, &command.to_le_bytes())?;
            self.state.lock().thread_mut(tid)?.completed.pop_front();
            return Ok(4);
        }
        // A target or a returned local object embeds userspace pointers. Hold
        // payload delivery until acquisition is acknowledged. Message leases
        // then preserve the relevant holds through decode and usercopy.
        if self.objects.lock().pending_local_acquisitions() {
            return Ok(0);
        }
        let thread = state.thread(tid).ok_or(errno::EPIPE)?;
        let replying =
            thread.pending_receive.is_none() && thread.outgoing().is_some_and(RpcCall::ready);
        let receiving = thread.can_receive(&self.endpoint);
        if thread.pending_delivery.is_none() && !replying && !receiving {
            return Ok(0);
        }
        if capacity < 68 {
            return Err(errno::ENOSPC);
        }
        let arena = state.arena.clone().ok_or(errno::EINVAL)?;
        let delivery = if let Some(delivery) = state.thread_mut(tid)?.pending_delivery.take() {
            delivery
        } else if replying {
            let message = match state.thread_mut(tid)?.stack.pop().ok_or(errno::EPIPE)? {
                Transaction::Outgoing(call) => call.take_reply().unwrap(),
                _ => unreachable!(),
            };
            Delivery {
                message,
                incoming: None,
                replying: true,
            }
        } else {
            match state.thread_mut(tid)?.receive(&self.endpoint) {
                Ok(Some(mut request)) => {
                    let message =
                        core::mem::replace(&mut request.message, objects::message_empty());
                    Delivery {
                        message: Ok(message),
                        incoming: Some(request),
                        replying: false,
                    }
                }
                _ => return Ok(0),
            }
        };
        let address = match arena.reserve(delivery.allocation_size()) {
            Ok(address) => address,
            Err(errno::ENOSPC) => {
                state.thread_mut(tid)?.pending_delivery = Some(delivery);
                return Ok(0);
            }
            Err(error) => return Err(error),
        };
        let Delivery {
            message,
            mut incoming,
            replying,
        } = delivery;
        let mut node = state.node;
        let mut context_requested = state.node_flags & 0x1000 != 0;
        drop(state);
        let result = (|| {
            let message = match message {
                Ok(message) => message,
                Err(_) => {
                    write(task, output, &BR_DEAD_REPLY.to_le_bytes())?;
                    return Ok(4);
                }
            };
            if let Some(request) = incoming.as_mut() {
                if let Some(key) = request.object_key() {
                    let dispatch = self.objects.lock().dispatch(key)?;
                    node = (dispatch.node.pointer, dispatch.node.cookie);
                    context_requested = dispatch.node.requests_context();
                    request.message.retained = Some(Arc::new(dispatch));
                }
            }
            let context_requested = context_requested && !replying;
            if context_requested && capacity < 76 {
                // Keep the delivered native request, route and its immutable
                // sender snapshot intact until the caller supplies room.
                let mut request = incoming.take().unwrap();
                request.message = message;
                self.state.lock().thread_mut(tid)?.pending_receive = Some(request);
                return Err(errno::ENOSPC);
            }
            let sender = incoming.as_ref().and_then(RpcRequest::sender);
            let context = if context_requested {
                let subject = sender
                    .and_then(|sender| sender.subject())
                    .ok_or(errno::EACCES)?;
                if !same_policy(self.subject.as_ref(), Some(subject)) {
                    return Err(errno::EACCES);
                }
                Some(subject.name())
            } else {
                None
            };
            if let Some(request) = incoming.as_ref() {
                enter_request_priority(request, task)?;
            }
            let mut installed = abi.map(|abi| buffers::Installed::new(task, abi));
            let (length, offsets_length, context_address) = self.decode_received_context(
                &message,
                address,
                &arena,
                installed.as_mut(),
                context,
            )?;
            let record_size = if context_address.is_some() { 76 } else { 68 };
            let mut record = [0u8; 76];
            record[..4].copy_from_slice(
                &(if replying {
                    BR_REPLY
                } else if context_address.is_some() {
                    BR_TRANSACTION_SEC_CTX
                } else {
                    BR_TRANSACTION
                })
                .to_le_bytes(),
            );
            let tx = &mut record[4..];
            if !replying {
                put64(tx, 0, node.0);
                put64(tx, 8, node.1);
            }
            tx[16..20].copy_from_slice(&message.opcode.to_le_bytes());
            tx[20..32].copy_from_slice(&message.bytes[..12]);
            let sender_pid = if !replying && u32_at(&message.bytes, 0) & 1 == 0 {
                sender
                    .and_then(|sender| task.get_namespace().resolve_local_id(sender.process_id()))
                    .and_then(|id| i32::try_from(id).ok())
                    .unwrap_or(0)
            } else {
                0
            };
            tx[24..28].copy_from_slice(&sender_pid.to_le_bytes());
            if let Some(sender) = sender {
                tx[28..32].copy_from_slice(&sender.principal().0.to_le_bytes());
            }
            put64(tx, 32, length);
            put64(tx, 40, offsets_length);
            put64(tx, 48, address);
            put64(tx, 56, address + ((length + 7) & !7));
            if let Some(context_address) = context_address {
                put64(tx, 64, context_address);
            }
            write(task, output, &record[..record_size])?;
            if let Some(installed) = installed {
                let automatic = installed.commit();
                if !automatic.is_empty() {
                    self.retain_auto_fds(address, automatic)?;
                }
            }
            if let Some(request) = incoming.take() {
                if request.is_oneway() {
                    let queued = self.objects.lock().queue_oneway(address, request);
                    if let Err((error, request)) = queued {
                        drop(request);
                        return Err(error);
                    }
                } else {
                    let mut state = self.state.lock();
                    let thread = state.thread_mut(tid)?;
                    thread.stack.push(Transaction::Incoming(Incoming {
                        accepts_fds: u32_at(&message.bytes, 0) & 0x10 != 0,
                        request,
                    }));
                }
            }
            Ok(record_size)
        })();
        if !matches!(result, Ok(68 | 76)) {
            let _ = self.release_buffer(&arena, address);
        }
        result
    }
    fn write_read(
        &self,
        task: &Task,
        abi: &LinuxAbi,
        tf: &mut Trapframe,
        arg: usize,
        nonblocking: bool,
    ) -> Result<(), usize> {
        let mut bwr = read::<48>(task, arg)?;
        let write_size = u64_at(&bwr, 0);
        let mut consumed = u64_at(&bwr, 8);
        let source = u64_at(&bwr, 16);
        let read_size = u64_at(&bwr, 24);
        let output = u64_at(&bwr, 40);
        if consumed > write_size
            || write_size > 1024 * 1024
            || read_size > 1024 * 1024
            || (read_size != 0 && read_size < 4)
            || u64_at(&bwr, 32) != 0
        {
            return Err(errno::EINVAL);
        }
        source.checked_add(write_size).ok_or(errno::EFAULT)?;
        output.checked_add(read_size).ok_or(errno::EFAULT)?;
        write(task, arg, &bwr)?;
        self.ensure_thread(task.get_id())?;
        let result = (|| {
            while consumed < write_size {
                if write_size - consumed < 4 {
                    return Err(errno::EINVAL);
                }
                let command = u32_at(&read::<4>(task, source + consumed)?, 0);
                let size = match command {
                    BC_TRANSACTION | BC_REPLY => 64,
                    BC_TRANSACTION_SG | BC_REPLY_SG => 72,
                    BC_FREE_BUFFER => 8,
                    BC_REQUEST_DEATH_NOTIFICATION | BC_CLEAR_DEATH_NOTIFICATION => 12,
                    BC_DEAD_BINDER_DONE => 8,
                    BC_INCREFS | BC_ACQUIRE | BC_RELEASE | BC_DECREFS => 4,
                    BC_INCREFS_DONE | BC_ACQUIRE_DONE => 16,
                    BC_ENTER_LOOPER | BC_REGISTER_LOOPER | BC_EXIT_LOOPER => 0,
                    _ => return Err(errno::EOPNOTSUPP),
                };
                if write_size - consumed < 4 + size {
                    return Err(errno::EINVAL);
                }
                let mut data = [0; 72];
                copy_from_user(task, source + consumed + 4, &mut data[..size])
                    .map_err(|_| errno::EFAULT)?;
                self.command(task, abi, command, &data[..size])?;
                consumed += 4 + size;
            }
            if read_size == 0 {
                return Ok(());
            }
            loop {
                let n = self.read_record_for_abi(task, Some(abi), output, read_size)?;
                if n != 0 {
                    put64(&mut bwr, 32, n);
                    return Ok(());
                }
                if nonblocking {
                    return Err(errno::EAGAIN);
                }
                let waker = self.waiter(task.get_id());
                {
                    let mut state = self.state.lock();
                    let thread = state.thread_mut(task.get_id())?;
                    thread.waiting_for_process = thread.accepts_process_work();
                }
                let waited =
                    waker.wait_result_until_owned(task.get_id(), tf, || self.ready(task.get_id()));
                {
                    let mut state = self.state.lock();
                    state.thread_mut(task.get_id())?.waiting_for_process = false;
                }
                if waited == WaitResult::Interrupted {
                    return Err(errno::EINTR);
                }
            }
        })();
        put64(&mut bwr, 8, consumed);
        write(task, arg, &bwr)?;
        result
    }
}

/// Tear down Binder files owned by a process before its shared descriptor table
/// is cleared. A retained `ioctl` Arc may still reference the file, so endpoint
/// and lease destruction must be explicit rather than waiting for final drop.
pub(super) fn process_exit(task: &Task) {
    let owner = task.get_thread_group_id();
    let files: Vec<_> = task
        .handle_table
        .active_handles()
        .into_iter()
        .filter_map(|handle| task.handle_table.get(handle))
        .filter(|object| from_object(object).is_some_and(|file| file.owner == owner))
        .collect();
    for object in files {
        if let Some(file) = from_object(&object) {
            file.close_owned_process();
        }
    }
}

pub(super) fn task_exit(task: &Task) {
    for handle in task.handle_table.active_handles() {
        if let Some(object) = task.handle_table.get(handle) {
            if let Some(file) = from_object(&object) {
                if file.owner == task.get_thread_group_id() {
                    file.thread_exit(task.get_id());
                }
            }
        }
    }
}

impl BinderFile {
    fn retain_auto_fds(
        &self,
        address: usize,
        descriptors: Vec<crate::abi::linux::generic::DescriptorReceipt>,
    ) -> Result<(), usize> {
        let state = self.state.lock();
        if state.closed {
            drop(state);
            drop(descriptors);
            return Err(errno::EPIPE);
        }
        let previous = {
            let mut fds = self.auto_fds.lock();
            fds.insert(address, descriptors)
        };
        drop(state);
        // Receipt teardown may close descriptors or wake waiters.
        drop(previous);
        Ok(())
    }

    fn close_owned_process(&self) {
        let send_gate = self.send_gate.lock();
        let (threads, arena) = {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.spawn = None;
            let mut threads = Vec::new();
            // Keep empty thread tombstones for ioctl callers already inside the
            // adapter; `closed` rejects those callers after their lock release.
            for thread in state.threads.values_mut() {
                threads.push(core::mem::take(thread));
            }
            (threads, state.arena.take())
        };
        drop(send_gate);
        let retained = {
            let mut objects = self.objects.lock();
            objects.close()
        };
        let auto_fds = {
            let mut fds = self.auto_fds.lock();
            core::mem::take(&mut *fds)
        };

        // Request cancellation, remote reference release and fd teardown can
        // all wake another endpoint. Keep them outside Binder locks.
        self.endpoint.close();
        drop(retained);
        drop(threads);
        drop(auto_fds);
        drop(arena);
        self.endpoint.waker().wake_all();
    }
}

impl Drop for BinderFile {
    fn drop(&mut self) {
        self.endpoint.close();
    }
}
impl StreamOps for BinderFile {
    fn read(&self, _: &mut [u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
    fn write(&self, _: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
}
impl ControlOps for BinderFile {}
impl MemoryMappingOps for BinderFile {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Binder mappings require process ownership")
    }
}
impl FileObject for BinderFile {
    fn seek(&self, _: SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::NotSupported)
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::CharDevice(crate::fs::DeviceFileInfo {
                device_id: self.domain.device as usize,
                device_type: crate::device::DeviceType::Char,
            }),
            size: 0,
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self as *const Self as usize as u64,
            link_count: 1,
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl Selectable for BinderFile {
    fn subscribe_readiness(
        &self,
        interest: ReadyInterest,
        waiter: &Arc<Waker>,
    ) -> Option<Vec<crate::sync::readiness::ReadinessRegistration>> {
        Some(if interest.read {
            alloc::vec![self.endpoint.waker().subscribe_readiness(waiter)]
        } else {
            Vec::new()
        })
    }
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        ReadySet {
            read: interest.read
                && crate::task::mytask().is_some_and(|task| self.ready(task.get_id())),
            write: false,
            except: false,
        }
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        timeout_ns: Option<u64>,
        minimum_ns: u64,
    ) -> SelectWaitOutcome {
        let Some(task) = crate::task::mytask() else {
            return SelectWaitOutcome::Interrupted;
        };
        if self.check_owner(&task).is_err() {
            return SelectWaitOutcome::Interrupted;
        }
        let tid = task.get_id();
        let started = crate::timer::get_time_ns();
        let minimum = timeout_ns.map_or(minimum_ns, |timeout| timeout.min(minimum_ns));
        if minimum > 0
            && !task.sleep_interruptible_with_precision(
                trapframe,
                minimum,
                crate::timer::TimerPrecision::Normal,
            )
        {
            return SelectWaitOutcome::Interrupted;
        }
        let ready = || interest.read && self.ready(tid);
        let waker = self.waiter(tid);
        let result = if let Some(timeout) = timeout_ns {
            let remaining = started
                .saturating_add(timeout)
                .saturating_sub(crate::timer::get_time_ns());
            waker.wait_with_timeout_result_until_owned(tid, trapframe, remaining, ready)
        } else {
            waker.wait_result_until_owned(tid, trapframe, ready)
        };
        match result {
            WaitResult::Woken => SelectWaitOutcome::Ready,
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskType;

    fn task(environment: &Arc<Environment>, id: usize) -> Task {
        let mut task = Task::new("binder-test".into(), 0, TaskType::User);
        task.set_id(id);
        task.set_namespace_id(id);
        *task.execution_environment.write() = Some(environment.clone());
        task
    }
    fn file(task: &Task, environment: &Arc<Environment>, device: u8) -> BinderFile {
        BinderFile {
            owner: task.get_thread_group_id(),
            subject: task.security_subject.read().clone(),
            domain: domain(environment.clone(), device),
            endpoint: RpcEndpoint::new(),
            send_gate: IrqSpinLock::new(()),
            auto_fds: IrqSpinLock::new(BTreeMap::new()),
            objects: IrqSpinLock::new(Objects::default()),
            state: IrqSpinLock::new(State {
                closed: false,
                max_threads: 0,
                spawn: None,
                threads: BTreeMap::new(),
                arena: None,
                node: (0, 0),
                node_flags: 0,
            }),
        }
    }
    #[test_case]
    fn binder_priority_donation_only_applies_to_fixed_priority_receivers() {
        let endpoint = RpcEndpoint::new();
        let caller = RpcEndpoint::new();
        let object = endpoint.create_object_with_scheduling(0, true).unwrap();
        let call = object
            .send_scoped_with_priority(
                RpcMessage {
                    opcode: 1,
                    bytes: Vec::new(),
                    objects: Vec::new(),
                    capabilities: Vec::new(),
                    retained: None,
                },
                &caller,
                None,
                8,
            )
            .unwrap();
        let request = endpoint.receive().unwrap().unwrap();
        let receiver = Task::new("binder-priority".into(), 0, TaskType::User);

        assert_eq!(enter_request_priority(&request, &receiver), Ok(()));
        assert_eq!(receiver.fixed_priority.effective(), 0);

        receiver.fixed_priority.configure(3);
        assert_eq!(enter_request_priority(&request, &receiver), Ok(()));
        assert_eq!(receiver.fixed_priority.effective(), 8);
        drop(call);
        assert_eq!(receiver.fixed_priority.effective(), 3);
    }
    fn context_fixture() -> (Task, Task, BinderFile, Arc<crate::shield::Policy>, usize) {
        let env = Environment::new();
        let server = task(&env, 301);
        let client = task(&env, 302);
        let policy = crate::shield::Policy::new(
            vec![
                (1, "u:r:rpc_server:s0".into()),
                (2, "u:r:rpc_client:s0".into()),
                (3, "u:r:rpc_later:s0".into()),
            ],
            vec![],
        )
        .unwrap();
        *server.security_subject.write() = Some(policy.subject(1).unwrap());
        *client.security_subject.write() = Some(policy.subject(2).unwrap());
        client
            .credentials
            .write()
            .set_principals(crate::shield::identity::Identities::all(
                crate::shield::identity::Principal(10042),
            ))
            .unwrap();
        let endpoint = file(&server, &env, 1);
        let base = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        server.allocate_stack_pages(base, 1).unwrap();
        let mut object = [0u8; 24];
        // Stock ProcessState supplies only flags; the header is zero.
        object[4..8].copy_from_slice(&0x1000u32.to_le_bytes());
        write(&server, base, &object).unwrap();
        assert_eq!(
            endpoint.ioctl(
                &server,
                &LinuxAbi::default(),
                &mut Trapframe::new(),
                SET_CONTEXT_EXT,
                base,
                false
            ),
            0
        );
        endpoint.ensure_thread(server.get_id()).unwrap();
        endpoint
            .state
            .lock()
            .threads
            .get_mut(&server.get_id())
            .unwrap()
            .looper = true;
        assert!(endpoint.mmap(&server, 0, 256 * 1024, 1, 2, 0) < usize::MAX - 4096);
        (server, client, endpoint, policy, base + 512)
    }
    fn context_message() -> RpcMessage {
        let mut message = objects::message_empty();
        message.bytes = vec![0; objects::HEADER];
        // An untrusted serialized euid cannot replace the native snapshot.
        message.bytes[8..12].copy_from_slice(&0u32.to_le_bytes());
        message
    }
    fn pool_limit(file: &BinderFile, task: &Task, output: usize, limit: u32) {
        write(task, output, &limit.to_le_bytes()).unwrap();
        assert_eq!(
            file.ioctl(
                task,
                &LinuxAbi::default(),
                &mut Trapframe::new(),
                SET_MAX_THREADS,
                output,
                false
            ),
            0
        );
    }

    #[test_case]
    fn binder_pool_spawn_is_bounded_retries_failed_copy_and_replaces_exited_workers() {
        let (server, client, file, _, output) = context_fixture();
        let abi = LinuxAbi::default();
        let peer = RpcEndpoint::new();
        let _call = file
            .endpoint
            .send_scoped_from(&client, context_message(), &peer, None)
            .unwrap();
        assert!(!file.state.lock().can_spawn(server.get_id()));
        pool_limit(&file, &server, output, 1);
        assert_eq!(file.read_record(&server, output, 3), Err(errno::ENOSPC));
        assert_eq!(file.read_record(&server, 0, 4), Err(errno::EFAULT));
        assert!(file.state.lock().can_spawn(server.get_id()));
        assert_eq!(file.read_record(&server, output, 4), Ok(4));
        assert_eq!(
            u32_at(&read::<4>(&server, output).unwrap(), 0),
            BR_SPAWN_LOOPER
        );
        assert!(!file.state.lock().can_spawn(server.get_id()));

        let worker = task(&file.domain.environment, 303);
        file.ensure_thread(worker.get_id()).unwrap();
        file.command(&worker, &abi, BC_REGISTER_LOOPER, &[])
            .unwrap();
        assert_eq!(
            file.command(&worker, &abi, BC_REGISTER_LOOPER, &[]),
            Err(errno::EINVAL)
        );
        assert!(!file.state.lock().can_spawn(server.get_id()));
        pool_limit(&file, &server, output, 2);
        file.state
            .lock()
            .thread_mut(worker.get_id())
            .unwrap()
            .waiting_for_process = true;
        assert!(!file.state.lock().can_spawn(server.get_id()));
        // An already waiting receiver can dispatch queued work without growth.
        assert!(file.ready(worker.get_id()));
        file.state
            .lock()
            .thread_mut(worker.get_id())
            .unwrap()
            .waiting_for_process = false;
        pool_limit(&file, &server, output, 1);
        file.command(&worker, &abi, BC_EXIT_LOOPER, &[]).unwrap();
        assert!(file.state.lock().can_spawn(server.get_id()));
        assert_eq!(file.read_record(&server, output, 4), Ok(4));
        file.command(&worker, &abi, BC_REGISTER_LOOPER, &[])
            .unwrap();
        file.thread_exit(worker.get_id());
        assert!(file.state.lock().can_spawn(server.get_id()));
        pool_limit(&file, &server, output, 0);
        assert!(!file.state.lock().can_spawn(server.get_id()));
        assert_eq!(
            file.command(&worker, &abi, BC_REGISTER_LOOPER, &[]),
            Err(errno::EPIPE)
        );
    }

    #[test_case]
    fn binder_pool_growth_breaks_unrelated_synchronous_call_cycle() {
        let environment = Environment::new();
        let mut first = task(&environment, 401);
        let mut second = task(&environment, 402);
        let a = file(&first, &environment, 0);
        let b = file(&second, &environment, 0);
        let abi = LinuxAbi::default();
        let output = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        for (file, task) in [(&a, &first), (&b, &second)] {
            file.ensure_thread(task.get_id()).unwrap();
            file.command(task, &abi, BC_ENTER_LOOPER, &[]).unwrap();
            task.allocate_stack_pages(output, 1).unwrap();
            assert!(file.mmap(task, 0, 256 * 1024, 1, 2, 0) < usize::MAX - 4096);
        }
        // These calls have different roots: neither may be delivered through
        // the other's synchronous callback stack. This is the SF/permission
        // service cycle when each process has only its original pool looper.
        let a_call = b
            .endpoint
            .send_scoped(context_message(), &a.endpoint, None)
            .unwrap();
        let b_call = a
            .endpoint
            .send_scoped(context_message(), &b.endpoint, None)
            .unwrap();
        a.state
            .lock()
            .thread_mut(401)
            .unwrap()
            .stack
            .push(Transaction::Outgoing(a_call));
        b.state
            .lock()
            .thread_mut(402)
            .unwrap()
            .stack
            .push(Transaction::Outgoing(b_call));
        for (file, task) in [(&a, &first), (&b, &second)] {
            assert!(!file.state.lock().threads[&task.get_id()].can_receive(&file.endpoint));
            assert!(!file.ready(task.get_id()));
            assert_eq!(file.read_record(task, output, 68), Ok(0));
            pool_limit(file, task, output, 1);
            assert!(file.ready(task.get_id()));
            assert_eq!(file.read_record(task, output, 68), Ok(4));
            assert_eq!(
                u32_at(&read::<4>(task, output).unwrap(), 0),
                BR_SPAWN_LOOPER
            );
        }
        first.set_id(403);
        second.set_id(404);
        for (file, task) in [(&a, &first), (&b, &second)] {
            file.ensure_thread(task.get_id()).unwrap();
            file.command(task, &abi, BC_REGISTER_LOOPER, &[]).unwrap();
            assert_eq!(file.read_record(task, output, 68), Ok(68));
            let record = read::<68>(task, output).unwrap();
            assert_eq!(u32_at(&record, 0), BR_TRANSACTION);
            let arena = file.state.lock().arena.clone().unwrap();
            file.release_buffer(&arena, u64_at(&record, 52)).unwrap();
            file.command(task, &abi, BC_REPLY, &[0; 64]).unwrap();
        }
        first.set_id(401);
        second.set_id(402);
        for (file, task) in [(&a, &first), (&b, &second)] {
            assert_eq!(file.read_record(task, output, 68), Ok(68));
            let record = read::<68>(task, output).unwrap();
            assert_eq!(u32_at(&record, 0), BR_REPLY);
            assert!(file.state.lock().threads[&task.get_id()].stack.is_empty());
            let arena = file.state.lock().arena.clone().unwrap();
            file.release_buffer(&arena, u64_at(&record, 52)).unwrap();
        }
    }

    #[test_case]
    fn binder_receive_backpressure_retains_native_request_until_space_is_freed() {
        let (server, client, endpoint, _, output) = context_fixture();
        let arena = endpoint.state.lock().arena.clone().unwrap();
        let occupied = arena.reserve(256 * 1024).unwrap();
        let caller = RpcEndpoint::new();
        let call = endpoint
            .endpoint
            .send_scoped_from(&client, context_message(), &caller, None)
            .unwrap();
        assert_eq!(endpoint.read_record(&server, output, 76), Ok(0));
        assert!(
            endpoint.state.lock().threads[&server.get_id()]
                .pending_delivery
                .is_some()
        );
        assert!(!endpoint.ready(server.get_id()));
        assert!(!call.ready());
        arena.free(occupied).unwrap();
        assert!(endpoint.ready(server.get_id()));
        assert_eq!(endpoint.read_record(&server, output, 76), Ok(76));
        assert!(
            endpoint.state.lock().threads[&server.get_id()]
                .pending_delivery
                .is_none()
        );
        let record = read::<76>(&server, output).unwrap();
        endpoint
            .release_buffer(&arena, u64_at(&record, 52))
            .unwrap();
    }
    #[test_case]
    fn binder_reply_backpressure_retains_completed_reply() {
        let (server, _, endpoint, _, output) = context_fixture();
        let arena = endpoint.state.lock().arena.clone().unwrap();
        let occupied = arena.reserve(256 * 1024).unwrap();
        let peer = RpcEndpoint::new();
        let call = peer
            .send_scoped(context_message(), &endpoint.endpoint, None)
            .unwrap();
        peer.receive()
            .unwrap()
            .unwrap()
            .reply(context_message())
            .unwrap();
        endpoint
            .state
            .lock()
            .threads
            .get_mut(&server.get_id())
            .unwrap()
            .stack
            .push(Transaction::Outgoing(call));
        assert_eq!(endpoint.read_record(&server, output, 68), Ok(0));
        assert!(!endpoint.ready(server.get_id()));
        arena.free(occupied).unwrap();
        assert!(endpoint.ready(server.get_id()));
        assert_eq!(endpoint.read_record(&server, output, 68), Ok(68));
        let record = read::<68>(&server, output).unwrap();
        assert_eq!(u32_at(&record, 0), BR_REPLY);
        endpoint
            .release_buffer(&arena, u64_at(&record, 52))
            .unwrap();
    }
    #[test_case]
    fn binder_sender_pid_uses_receiver_namespace_and_never_serialized_identity() {
        let (server, client, endpoint, _policy, output) = context_fixture();
        let namespace = crate::task::namespace::TaskNamespace::new_root("binder-receiver".into());
        server.set_namespace(namespace.clone());
        let unrelated = namespace.allocate_task_id_for(909);
        let local = namespace.allocate_task_id_for(client.get_thread_group_id());
        assert_ne!(local, client.get_namespace_id());
        let caller = RpcEndpoint::new();
        for visible in [true, false] {
            if !visible {
                namespace.unregister_mapping_for_global(client.get_thread_group_id());
            }
            let mut message = context_message();
            message.bytes[4..8].copy_from_slice(&(unrelated as u32).to_le_bytes());
            let call = endpoint
                .endpoint
                .send_scoped_from(&client, message, &caller, None)
                .unwrap();
            assert_eq!(endpoint.read_record(&server, output, 76), Ok(76));
            let record = read::<76>(&server, output).unwrap();
            assert_eq!(u32_at(&record, 28), if visible { local as u32 } else { 0 });
            assert_eq!(u32_at(&record, 0), BR_TRANSACTION_SEC_CTX);
            let pointer = u64_at(&record, 68);
            assert_eq!(
                read::<18>(&server, pointer).unwrap(),
                *b"u:r:rpc_client:s0\0"
            );
            endpoint
                .command(&server, &LinuxAbi::default(), BC_REPLY, &[0; 64])
                .unwrap();
            assert!(call.take_reply().unwrap().is_ok());
            endpoint
                .release_buffer(
                    &endpoint.state.lock().arena.clone().unwrap(),
                    u64_at(&record, 52),
                )
                .unwrap();
            assert_eq!(endpoint.read_record(&server, output, 76), Ok(4));
            assert_eq!(u32_at(&read::<4>(&server, output).unwrap(), 0), BR_COMPLETE);
        }
    }
    #[test_case]
    fn binder_context_records_use_native_sender_snapshot_and_read_only_lifetime() {
        let (server, client, endpoint, policy, output) = context_fixture();
        let caller = RpcEndpoint::new();
        let call = endpoint
            .endpoint
            .send_scoped_from(&client, context_message(), &caller, None)
            .unwrap();
        *client.security_subject.write() = Some(policy.subject(3).unwrap());
        assert_eq!(
            endpoint.read_record(&server, output, 68),
            Err(errno::ENOSPC)
        );
        assert!(!call.ready());
        assert!(endpoint.ready(server.get_id()));
        assert_eq!(endpoint.read_record(&server, output, 76), Ok(76));
        let record = read::<76>(&server, output).unwrap();
        assert_eq!(u32_at(&record, 0), BR_TRANSACTION_SEC_CTX);
        assert_eq!(u32_at(&record, 32), 10042);
        let pointer = u64_at(&record, 68);
        let buffer = u64_at(&record, 52);
        assert_eq!(
            read::<18>(&server, pointer).unwrap(),
            *b"u:r:rpc_client:s0\0"
        );
        assert!(copy_to_user(&server, pointer, b"forged").is_err());
        endpoint
            .command(&server, &LinuxAbi::default(), BC_REPLY, &[0; 64])
            .unwrap();
        assert!(call.take_reply().unwrap().is_ok());
        endpoint
            .release_buffer(&endpoint.state.lock().arena.clone().unwrap(), buffer)
            .unwrap();
        assert_eq!(read::<18>(&server, pointer).unwrap(), [0; 18]);
    }
    #[test_case]
    fn binder_context_retry_cancellation_and_failed_usercopy_release_requests() {
        let (server, client, endpoint, _policy, output) = context_fixture();
        let caller = RpcEndpoint::new();
        let call = endpoint
            .endpoint
            .send_scoped_from(&client, context_message(), &caller, None)
            .unwrap();
        assert_eq!(
            endpoint.read_record(&server, output, 68),
            Err(errno::ENOSPC)
        );
        drop(call);
        assert_eq!(endpoint.read_record(&server, output, 76), Ok(0));
        assert!(
            endpoint.state.lock().threads[&server.get_id()]
                .pending_receive
                .is_none()
        );
        let call = endpoint
            .endpoint
            .send_scoped_from(&client, context_message(), &caller, None)
            .unwrap();
        assert_eq!(endpoint.read_record(&server, 0, 76), Err(errno::EFAULT));
        assert_eq!(
            call.take_reply().unwrap().err(),
            Some(crate::ipc::rpc::RpcError::PeerClosed)
        );
        let arena = endpoint.state.lock().arena.clone().unwrap();
        let address = arena.reserve(8).unwrap();
        arena.free(address).unwrap();
    }
    #[test_case]
    fn binder_context_delivery_rejects_absent_and_foreign_native_labels() {
        let (server, client, endpoint, _policy, output) = context_fixture();
        let caller = RpcEndpoint::new();
        for foreign in [false, true] {
            *client.security_subject.write() = if foreign {
                Some(
                    crate::shield::Policy::new(vec![(1, "u:r:rpc_client:s0".into())], vec![])
                        .unwrap()
                        .subject(1)
                        .unwrap(),
                )
            } else {
                None
            };
            let call = endpoint
                .endpoint
                .send_scoped_from(&client, context_message(), &caller, None)
                .unwrap();
            assert_eq!(
                endpoint.read_record(&server, output, 76),
                Err(errno::EACCES)
            );
            assert_eq!(
                call.take_reply().unwrap().err(),
                Some(crate::ipc::rpc::RpcError::PeerClosed)
            );
        }
    }
    #[test_case]
    fn binder_nested_calls_restore_reply_targets_and_fd_policy_after_reentry() {
        let environment = Environment::new();
        let sender = task(&environment, 1);
        let receiver = task(&environment, 2);
        let manager_task = task(&environment, 3);
        let a = file(&sender, &environment, 0);
        let b = file(&receiver, &environment, 0);
        let manager = file(&manager_task, &environment, 0);
        let abi = LinuxAbi::default();
        assert_eq!(
            manager.ioctl(
                &manager_task,
                &abi,
                &mut Trapframe::new(),
                SET_CONTEXT,
                0,
                false
            ),
            0
        );
        b.ensure_thread(2).unwrap();
        assert!(b.mmap(&receiver, 0, 256 * 1024, 1, 2, 0) < usize::MAX - 4096);
        let output = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        receiver.allocate_stack_pages(output, 1).unwrap();
        let outer = b
            .endpoint
            .send_scoped(objects::message_empty(), &a.endpoint, None)
            .unwrap();
        b.state
            .lock()
            .threads
            .get_mut(&2)
            .unwrap()
            .stack
            .push(Transaction::Incoming(Incoming {
                request: b.endpoint.receive().unwrap().unwrap(),
                accepts_fds: true,
            }));
        let mut transaction = [0u8; 64];
        transaction[16..20].copy_from_slice(&41u32.to_le_bytes());
        b.command(&receiver, &abi, BC_TRANSACTION, &transaction)
            .unwrap();
        assert_eq!(b.state.lock().threads[&2].stack.len(), 2);
        assert_eq!(b.read_record(&receiver, output, 68), Ok(4));
        let at_manager = manager.endpoint.receive().unwrap().unwrap();
        let mut callback_message = at_manager.message.clone();
        callback_message.opcode = 42;
        let callback = b
            .endpoint
            .send_scoped(
                callback_message,
                &manager.endpoint,
                at_manager.context().as_ref(),
            )
            .unwrap();
        // The waiting caller is not a registered looper. Its callback is routed
        // to its outstanding call rather than requiring another pool thread.
        assert!(!b.state.lock().threads[&2].looper);
        assert!(b.ready(2));
        assert_eq!(b.read_record(&receiver, output, 68), Ok(68));
        assert_eq!(
            u32_at(&read::<68>(&receiver, output).unwrap(), 0),
            BR_TRANSACTION
        );
        assert!(!b.state.lock().threads[&2].incoming().unwrap().accepts_fds);
        b.command(&receiver, &abi, BC_REPLY, &transaction).unwrap();
        assert!(callback.take_reply().unwrap().is_ok());
        assert_eq!(b.read_record(&receiver, output, 68), Ok(4));
        let response = at_manager.message.clone();
        at_manager.reply(response).unwrap();
        assert_eq!(b.read_record(&receiver, output, 68), Ok(68));
        assert_eq!(u32_at(&read::<68>(&receiver, output).unwrap(), 0), BR_REPLY);
        assert!(b.state.lock().threads[&2].incoming().unwrap().accepts_fds);
        b.command(&receiver, &abi, BC_REPLY, &transaction).unwrap();
        assert!(outer.take_reply().unwrap().is_ok());
        assert!(b.state.lock().threads[&2].stack.is_empty());
    }

    #[test_case]
    fn binder_task_exit_cancels_transactions_through_vfs_wrappers() {
        use crate::fs::vfs_v2::{core::VfsFileObject, drivers::tmpfs::TmpFS, manager::VfsManager};
        let environment = Environment::new();
        let task = task(&environment, 1);
        let server = RpcEndpoint::new();
        let binder = Arc::new(file(&task, &environment, 0));
        binder.ensure_thread(1).unwrap();
        let call = server
            .send_scoped(objects::message_empty(), &binder.endpoint, None)
            .unwrap();
        let request = server.receive().unwrap().unwrap();
        binder
            .state
            .lock()
            .threads
            .get_mut(&1)
            .unwrap()
            .stack
            .push(Transaction::Outgoing(call));
        let view = VfsManager::new_with_root(TmpFS::new(0));
        let (entry, mount) = view.resolve_path("/").unwrap();
        let wrapped = Arc::new(VfsFileObject::new(
            binder.clone(),
            entry,
            mount,
            "/dev/binder".into(),
        ));
        task.handle_table
            .insert(KernelObject::File(wrapped))
            .unwrap();
        task_exit(&task);
        assert!(request.cancelled());
        assert!(binder.state.lock().threads.is_empty());
    }

    #[test_case]
    fn binder_descriptor_flush_returns_each_waiter_without_revoking_rpc() {
        let environment = Environment::new();
        let first = task(&environment, 1);
        let second = task(&environment, 2);
        let file = Arc::new(file(&first, &environment, 0));
        let object = KernelObject::File(file.clone());
        file.ensure_thread(1).unwrap();
        file.ensure_thread(2).unwrap();
        assert!(!file.ready(1) && !file.ready(2));
        let abi = LinuxAbi::with_extensions(&crate::abi::android::AndroidExtensions);
        abi.close_descriptor_object(&first, &object);
        assert!(!file.state.lock().closed);
        assert!(file.ready(1) && file.ready(2));
        assert_eq!(file.read_record(&first, 0, 4), Err(errno::EFAULT));
        assert!(file.ready(1));
        let output = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        for task in [&first, &second] {
            task.allocate_stack_pages(output, 1).unwrap();
            assert_eq!(file.read_record(task, output, 3), Err(errno::ENOSPC));
            assert_eq!(file.read_record(task, output, 4), Ok(4));
            assert_eq!(u32_at(&read::<4>(task, output).unwrap(), 0), BR_NOOP);
            assert!(!file.ready(task.get_id()));
            assert_eq!(file.read_record(task, output, 4), Ok(0));
        }
        // A duplicate/retained descriptor continues using the same endpoint.
        let call = file.endpoint.send(objects::message_empty()).unwrap();
        file.endpoint
            .receive()
            .unwrap()
            .unwrap()
            .reply(objects::message_empty())
            .unwrap();
        assert!(call.ready());
        abi.close_descriptor_object(&first, &object);
        assert!(file.ready(1) && file.ready(2));
    }

    #[test_case]
    fn binder_oneway_completion_and_serialization_follow_receive_buffer_lifetime() {
        let environment = Environment::new();
        let sender = task(&environment, 1);
        let receiver = task(&environment, 2);
        let client = file(&sender, &environment, 0);
        let server = file(&receiver, &environment, 0);
        let abi = LinuxAbi::default();
        assert_eq!(
            server.ioctl(
                &receiver,
                &abi,
                &mut Trapframe::new(),
                SET_CONTEXT,
                0,
                false
            ),
            0
        );
        client.ensure_thread(1).unwrap();
        server.ensure_thread(2).unwrap();
        server
            .command(&receiver, &abi, BC_ENTER_LOOPER, &[])
            .unwrap();
        assert!(server.mmap(&receiver, 0, 256 * 1024, 1, 2, 0) < usize::MAX - 4096);
        let output = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        receiver.allocate_stack_pages(output, 1).unwrap();
        // An oneway callback is allowed while this thread handles a sync call.
        let call = client.endpoint.send(objects::message_empty()).unwrap();
        client
            .state
            .lock()
            .threads
            .get_mut(&1)
            .unwrap()
            .stack
            .push(Transaction::Incoming(Incoming {
                request: client.endpoint.receive().unwrap().unwrap(),
                accepts_fds: false,
            }));
        let mut data = [0u8; 64];
        data[20..24].copy_from_slice(&1u32.to_le_bytes());
        client
            .command(&sender, &abi, BC_TRANSACTION, &data)
            .unwrap();
        assert!(client.state.lock().threads[&1].outgoing().is_none());
        assert!(client.state.lock().threads[&1].incoming().is_some());
        assert_eq!(
            client.state.lock().threads[&1]
                .completed
                .front()
                .unwrap()
                .command,
            BR_COMPLETE
        );
        // A failed delivery cannot strand the per-object gate or its budget.
        assert_eq!(server.read_record(&receiver, 0, 68), Err(errno::EFAULT));
        for _ in 0..2 {
            client
                .command(&sender, &abi, BC_TRANSACTION, &data)
                .unwrap();
        }
        for _ in 0..2 {
            assert!(server.ready(2));
            assert_eq!(server.read_record(&receiver, output, 68), Ok(68));
            let record = read::<68>(&receiver, output).unwrap();
            assert_eq!(u32_at(&record, 0), BR_TRANSACTION);
            assert_eq!(u32_at(&record, 24), 1);
            assert_eq!(u32_at(&record, 28), 0); // Linux oneway sender PID semantics
            assert!(server.state.lock().threads[&2].incoming().is_none());
            assert!(!server.ready(2));
            let arena = server.state.lock().arena.clone().unwrap();
            let address = u64_at(&record, 52);
            assert_eq!(
                server.release_buffer(&arena, address + 8),
                Err(errno::EINVAL)
            );
            assert!(!server.ready(2));
            let mut reply = data;
            reply[20..24].fill(0);
            assert_eq!(
                server.command(&receiver, &abi, BC_REPLY, &reply),
                Err(errno::EINVAL)
            );
            server.release_buffer(&arena, address).unwrap();
        }
        assert!(server.objects.lock().oneway.is_empty());
        assert!(!call.ready());
    }
    #[test_case]
    fn binder_transports_framework_appops_flag_without_accepting_unknown_flags() {
        let environment = Environment::new();
        let sender = task(&environment, 1);
        let receiver = task(&environment, 2);
        let client = file(&sender, &environment, 0);
        let server = file(&receiver, &environment, 0);
        let abi = LinuxAbi::default();
        assert_eq!(
            server.ioctl(
                &receiver,
                &abi,
                &mut Trapframe::new(),
                SET_CONTEXT,
                0,
                false
            ),
            0
        );
        let output = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        for (file, task) in [(&client, &sender), (&server, &receiver)] {
            file.ensure_thread(task.get_id()).unwrap();
            assert!(file.mmap(task, 0, 256 * 1024, 1, 2, 0) < usize::MAX - 4096);
            task.allocate_stack_pages(output, 1).unwrap();
        }
        server
            .command(&receiver, &abi, BC_ENTER_LOOPER, &[])
            .unwrap();
        let mut data = [0; 64];
        data[20..24].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(
            client.command(&sender, &abi, BC_TRANSACTION, &data),
            Err(errno::EOPNOTSUPP)
        );
        assert!(!server.ready(2));
        data[20..24].copy_from_slice(&0x12u32.to_le_bytes());
        client
            .command(&sender, &abi, BC_TRANSACTION, &data)
            .unwrap();
        assert_eq!(server.read_record(&receiver, output, 68), Ok(68));
        let record = read::<68>(&receiver, output).unwrap();
        assert_eq!(u32_at(&record, 0), BR_TRANSACTION);
        assert_eq!(u32_at(&record, 24), FLAG_COLLECT_NOTED_APP_OPS | 0x10);
        assert_eq!(u32_at(&record, 28), 1);
        server.command(&receiver, &abi, BC_REPLY, &data).unwrap();
        assert_eq!(client.read_record(&sender, output, 68), Ok(4));
        assert_eq!(u32_at(&read::<4>(&sender, output).unwrap(), 0), BR_COMPLETE);
        assert_eq!(client.read_record(&sender, output, 68), Ok(68));
        let record = read::<68>(&sender, output).unwrap();
        assert_eq!(u32_at(&record, 0), BR_REPLY);
        assert_eq!(u32_at(&record, 24), FLAG_COLLECT_NOTED_APP_OPS | 0x10);
    }
    #[test_case]
    fn binder_context_death_checks_wire_cookie_and_preserves_failed_usercopy() {
        let environment = Environment::new();
        let task = task(&environment, 1);
        let server = file(&task, &environment, 0);
        let client = file(&task, &environment, 0);
        let abi = LinuxAbi::default();
        let mut tf = Trapframe::new();
        assert_eq!(server.ioctl(&task, &abi, &mut tf, SET_CONTEXT, 0, false), 0);
        client.ensure_thread(1).unwrap();
        client.command(&task, &abi, BC_ENTER_LOOPER, &[]).unwrap();
        client
            .command(&task, &abi, BC_INCREFS, &0u32.to_le_bytes())
            .unwrap();
        let mut data = [0; 12];
        put64(&mut data, 4, 0x1234_5678_8765_4321u64 as usize);
        client
            .command(&task, &abi, BC_REQUEST_DEATH_NOTIFICATION, &data)
            .unwrap();
        assert!(!client.ready(1));
        drop(server);
        assert!(client.ready(1));
        assert_eq!(client.read_record(&task, 0, 11), Err(errno::ENOSPC));
        assert_eq!(client.read_record(&task, 0, 12), Err(errno::EFAULT));
        assert!(client.ready(1));
        // Dropping the last handle reference cancels its attached death work.
        client
            .command(&task, &abi, BC_DECREFS, &0u32.to_le_bytes())
            .unwrap();
        assert!(!client.ready(1));
    }

    #[test_case]
    fn binder_oneway_completion_precedes_process_death_with_fault_retry() {
        let environment = Environment::new();
        let sender = task(&environment, 1);
        let receiver = task(&environment, 2);
        let client = file(&sender, &environment, 0);
        let server = file(&receiver, &environment, 0);
        let abi = LinuxAbi::default();
        assert_eq!(
            server.ioctl(
                &receiver,
                &abi,
                &mut Trapframe::new(),
                SET_CONTEXT,
                0,
                false
            ),
            0
        );
        client.ensure_thread(sender.get_id()).unwrap();
        client.command(&sender, &abi, BC_ENTER_LOOPER, &[]).unwrap();
        client
            .command(&sender, &abi, BC_INCREFS, &0u32.to_le_bytes())
            .unwrap();
        let mut death = [0; 12];
        put64(&mut death, 4, 0x1234_5678_8765_4321u64 as usize);
        client
            .command(&sender, &abi, BC_REQUEST_DEATH_NOTIFICATION, &death)
            .unwrap();

        // Queue one-way thread completion first, then make process-level
        // death work ready. Linux still gives the thread completion priority.
        let mut transaction = [0; 64];
        transaction[20..24].copy_from_slice(&1u32.to_le_bytes());
        client
            .command(&sender, &abi, BC_TRANSACTION, &transaction)
            .unwrap();
        drop(server);

        assert!(client.ready(sender.get_id()));
        // A failed completion usercopy consumes neither the completion nor the
        // queued death and must not select the latter out of order.
        assert_eq!(client.read_record(&sender, 0, 4), Err(errno::EFAULT));
        assert!(client.ready(sender.get_id()));
        let output = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        sender.allocate_stack_pages(output, 1).unwrap();
        assert_eq!(client.read_record(&sender, output, 4), Ok(4));
        assert_eq!(u32_at(&read::<4>(&sender, output).unwrap(), 0), BR_COMPLETE);
        assert!(client.ready(sender.get_id()));
        assert_eq!(client.read_record(&sender, output, 12), Ok(12));
        assert_eq!(
            u32_at(&read::<12>(&sender, output).unwrap(), 0),
            death::BR_DEAD_BINDER
        );
    }
    #[test_case]
    fn binder_shield_domain_and_relabel_cannot_reuse_foreign_authority() {
        let environment = Environment::new();
        let policy =
            crate::shield::Policy::new(vec![(1, "server".into()), (2, "client".into())], vec![])
                .unwrap();
        let server_task = task(&environment, 1);
        *server_task.security_subject.write() = Some(policy.subject(1).unwrap());
        let server = file(&server_task, &environment, 0);
        let client_task = task(&environment, 2);
        *client_task.security_subject.write() = Some(policy.subject(2).unwrap());
        let client = file(&client_task, &environment, 0);
        let foreign_task = task(&environment, 3);
        let foreign = file(&foreign_task, &environment, 0);
        let abi = LinuxAbi::default();
        let mut tf = Trapframe::new();
        assert_eq!(
            server.ioctl(&server_task, &abi, &mut tf, SET_CONTEXT, 0, false),
            0
        );
        for (file, task) in [(&client, &client_task), (&foreign, &foreign_task)] {
            file.ensure_thread(task.get_id()).unwrap();
        }
        assert_eq!(
            foreign.command(&foreign_task, &abi, BC_TRANSACTION, &[0; 64]),
            Err(errno::EACCES)
        );
        foreign
            .command(&foreign_task, &abi, BC_INCREFS, &0u32.to_le_bytes())
            .unwrap();
        assert_eq!(
            foreign.command(&foreign_task, &abi, BC_REQUEST_DEATH_NOTIFICATION, &[0; 12]),
            Err(errno::EACCES)
        );
        assert!(
            client
                .command(&client_task, &abi, BC_TRANSACTION, &[0; 64])
                .is_ok()
        );
        assert!(server.endpoint.receive().unwrap().is_some());
        *client_task.security_subject.write() = Some(policy.subject(1).unwrap());
        assert_eq!(client.check_owner(&client_task), Err(errno::EINVAL));
        *client_task.security_subject.write() = None;
        assert_eq!(client.check_owner(&client_task), Err(errno::EINVAL));
    }
    #[test_case]
    fn binder_routes_only_within_the_same_environment_and_device() {
        let a = Environment::new();
        let b = Environment::new();
        let ta = task(&a, 1);
        let tb = task(&b, 2);
        // Same PID namespace, separate execution environments.
        assert!(Arc::ptr_eq(&ta.get_namespace(), &tb.get_namespace()));
        let server = file(&ta, &a, 0);
        let client = file(&ta, &a, 0);
        let foreign = file(&tb, &b, 0);
        let hardware = file(&ta, &a, 1);
        let vendor = file(&ta, &a, 2);
        let abi = LinuxAbi::default();
        let mut tf = Trapframe::new();
        assert_eq!(server.ioctl(&ta, &abi, &mut tf, SET_CONTEXT, 0, false), 0);
        assert_eq!(
            client.ioctl(&ta, &abi, &mut tf, SET_CONTEXT, 0, false),
            errno::to_result(errno::EBUSY)
        );
        for (file, task) in [(&foreign, &tb), (&hardware, &ta), (&vendor, &ta)] {
            file.ensure_thread(task.get_id()).unwrap();
            file.command(task, &abi, BC_TRANSACTION, &[0; 64]).unwrap();
            assert_eq!(
                file.state.lock().threads[&task.get_id()]
                    .completed
                    .front()
                    .map(|completed| completed.command),
                Some(BR_DEAD_REPLY)
            );
            assert!(server.endpoint.receive().unwrap().is_none());
            assert_eq!(file.ioctl(task, &abi, &mut tf, SET_CONTEXT, 0, false), 0);
        }
        client.ensure_thread(ta.get_id()).unwrap();
        client.command(&ta, &abi, BC_TRANSACTION, &[0; 64]).unwrap();
        assert!(server.endpoint.receive().unwrap().is_some());
        assert!(foreign.endpoint.receive().unwrap().is_none());
    }
    #[test_case]
    fn binder_rejects_a_handle_after_its_process_changes_environment() {
        let a = Environment::new();
        let task = task(&a, 1);
        let file = file(&task, &a, 0);
        assert_eq!(file.check_owner(&task), Ok(()));
        *task.execution_environment.write() = Some(Environment::new());
        assert_eq!(file.check_owner(&task), Err(errno::EINVAL));
    }
    #[test_case]
    fn binder_rejects_invalid_write_read_lengths_without_consuming_commands() {
        let environment = Environment::new();
        let task = task(&environment, 1);
        let file = file(&task, &environment, 0);
        let base = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE * 2;
        task.allocate_stack_pages(base, 2).unwrap();
        let arg = base + crate::environment::PAGE_SIZE - 16;
        let mut bwr = [0; 48];
        put64(&mut bwr, 24, 2);
        write(&task, arg, &bwr).unwrap();
        assert_eq!(
            file.write_read(
                &task,
                &LinuxAbi::default(),
                &mut Trapframe::new(),
                arg,
                true
            ),
            Err(errno::EINVAL)
        );
        put64(&mut bwr, 24, 0);
        put64(&mut bwr, 0, 3);
        put64(&mut bwr, 16, base);
        write(&task, arg, &bwr).unwrap();
        assert_eq!(
            file.write_read(
                &task,
                &LinuxAbi::default(),
                &mut Trapframe::new(),
                arg,
                true
            ),
            Err(errno::EINVAL)
        );
        assert_eq!(u64_at(&read::<48>(&task, arg).unwrap(), 8), 0);
    }
}
