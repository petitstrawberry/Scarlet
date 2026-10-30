//! Initial 64-bit Binder adapter over native RPC objects. Supports bounded,
//! synchronous transactions, object references, descriptors and HIDL embedded
//! buffers over native RPC. Nested synchronous transactions remain unsupported.
//! One-way delivery is serialized until userspace frees each receive buffer.

use crate::{
    abi::linux::generic::{LinuxAbi, errno, fs::FD_CLOEXEC},
    arch::Trapframe,
    executor::environment::Environment,
    fs::{FileMetadata, FilePermission, FileType, SeekFrom},
    ipc::rpc::{RpcCall, RpcEndpoint, RpcMessage, RpcRequest},
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
const BC_TRANSACTION_SG:u32=0x40486311;
const BC_REPLY_SG:u32=0x40486312;
const BC_FREE_BUFFER: u32 = 0x40086303;
const BC_REGISTER_LOOPER: u32 = 0x630b;
const BC_ENTER_LOOPER: u32 = 0x630c;
const BC_EXIT_LOOPER: u32 = 0x630d;
const BR_TRANSACTION: u32 = 0x80407202;
const BR_REPLY: u32 = 0x80407203;
const BR_DEAD_REPLY: u32 = 0x7205;
const BR_COMPLETE: u32 = 0x7206;
const BR_NOOP: u32 = 0x720c;
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
    device:u8,
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
    flush_generation: u64,
    returned_flush: u64,
    completed: VecDeque<Completed>,
    outgoing: Option<RpcCall>,
    incoming: Option<RpcRequest>,
    incoming_accepts_fds: bool,
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
struct State {
    threads: BTreeMap<usize, Thread>,
    arena: Option<Arc<Arena>>,
    node: (usize, usize),
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

pub(crate) fn open(abi: &mut LinuxAbi, task: &Task, path: &str, flags: i32) -> Option<usize> {
    let device = match path {
        "/dev/binder" => 0,
        "/dev/hwbinder" => 1,
        "/dev/vndbinder" => 2,
        _ => return None,
    };
    let result = (|| {
        if flags & 3 != 2 {
            return Err(errno::EACCES);
        }
        if flags & 0x10000 != 0 {
            return Err(errno::ENOTDIR);
        }
        if flags & !(3 | 0x80000 | 0x800 | 0x20000) != 0 {
            return Err(errno::EINVAL);
        }
        let environment = task
            .execution_environment
            .read()
            .clone()
            .ok_or(errno::ENODEV)?;
        let file = Arc::new(BinderFile {
            owner: task.get_thread_group_id(),
            subject: task.security_subject.read().clone(),
            domain: domain(environment, device),
            endpoint: RpcEndpoint::new(),
            send_gate: IrqSpinLock::new(()),
            auto_fds: IrqSpinLock::new(BTreeMap::new()),
            objects: IrqSpinLock::new(Objects::default()),
            state: IrqSpinLock::new(State {
                threads: BTreeMap::new(),
                arena: None,
                node: (0, 0),
            }),
        });
        let handle = task
            .handle_table
            .insert(KernelObject::File(file))
            .map_err(|_| errno::EMFILE)?;
        let fd = match abi.allocate_fd(handle) {
            Ok(fd) => fd,
            Err(_) => {
                let _ = task.handle_table.remove(handle);
                return Err(errno::EMFILE);
            }
        };
        let _ = abi.set_file_status_flags(fd, flags as u32 & 0x803);
        if flags & 0x80000 != 0 {
            let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
        }
        Ok(fd)
    })();
    Some(result.unwrap_or_else(errno::to_result))
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
            Ok(())
        } else {
            Err(errno::EINVAL)
        }
    }
    fn ensure_thread(&self, tid: usize) -> Result<(), usize> {
        let mut state = self.state.lock();
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
            match request {
                VERSION => write(task, arg, &8i32.to_le_bytes()),
                SET_MAX_THREADS => {
                    let n = u32_at(&read::<4>(task, arg)?, 0);
                    if n as usize > MAX_THREADS {
                        Err(errno::EINVAL)
                    } else {
                        Ok(())
                    }
                }
                SET_CONTEXT | SET_CONTEXT_EXT => {
                    let node = if request == SET_CONTEXT_EXT {
                        let object = read::<24>(task, arg)?;
                        // Security-context records and scheduler policy need their
                        // own implementation; do not claim to support these flags.
                        if u32_at(&object, 0) != 0x73622a85 || u32_at(&object, 4) & !0x100 != 0 {
                            return Err(errno::EOPNOTSUPP);
                        }
                        (u64_at(&object, 8), u64_at(&object, 16))
                    } else {
                        (0, 0)
                    };
                    let mut context = self.domain.context.lock();
                    if context
                        .as_ref()
                        .and_then(|c| c.endpoint.upgrade())
                        .is_some()
                    {
                        return Err(errno::EBUSY);
                    }
                    self.state.lock().node = node;
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
        let command=match command {BC_TRANSACTION_SG=>BC_TRANSACTION,BC_REPLY_SG=>BC_REPLY,other=>other};
        let tid = task.get_id();
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
                    .threads
                    .get(&tid)
                    .unwrap()
                    .looper
                    .then_some(tid);
                let mut objects = self.objects.lock();
                if command == BC_CLEAR_DEATH_NOTIFICATION {
                    objects
                        .deaths
                        .clear(u32_at(data, 0), u64_at(data, 4), target)?;
                } else {
                    objects.deaths.done(u64_at(data, 0), target)?;
                }
            }
            BC_ENTER_LOOPER | BC_REGISTER_LOOPER | BC_EXIT_LOOPER => {
                self.state.lock().threads.get_mut(&tid).unwrap().looper = command != BC_EXIT_LOOPER;
            }
            BC_FREE_BUFFER => {
                let arena = self.state.lock().arena.clone().ok_or(errno::EINVAL)?;
                self.release_buffer(&arena, u64_at(data, 0))?;
            }
            BC_INCREFS | BC_ACQUIRE | BC_RELEASE | BC_DECREFS | BC_INCREFS_DONE
            | BC_ACQUIRE_DONE => {
                self.objects.lock().reference_command(command, data)?;
            }
            BC_TRANSACTION | BC_REPLY => {
                // hwbinder's default INHERIT_RT flag is inert between Fair
                // tasks. Do not turn a native reservation into an unimplemented
                // inherited FIFO/RR transaction.
                if self.domain.device==1 && task.deadline_enabled() {return Err(errno::EOPNOTSUPP);}
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
                let target = if command == BC_TRANSACTION && number != 0 {
                    Some(self.objects.lock().target(number as u32)?)
                } else {
                    None
                };
                let capability = target.as_ref().map(|lease| lease.node.object.clone());
                if task.deadline_enabled() && target.as_ref().is_some_and(|target|target.node.inherits_rt()) {return Err(errno::EOPNOTSUPP);}
                if !prepared.capabilities.is_empty() {
                    let accepts=if command==BC_TRANSACTION {
                        target.as_ref().is_some_and(|target|target.node.accepts_fds())
                    } else {
                        self.state.lock().threads.get(&tid).is_some_and(|thread|thread.incoming_accepts_fds)
                    };
                    if !accepts {return Err(errno::EPERM);}
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
                    let thread = state.threads.get_mut(&tid).unwrap();
                    if !oneway && (thread.outgoing.is_some() || thread.incoming.is_some()) {
                        return Err(errno::EOPNOTSUPP);
                    }
                    if thread.completed.len() >= 64 {
                        return Err(errno::EAGAIN);
                    }
                    if oneway {
                        let sent = if let Some(capability) = capability {
                            capability.send_oneway(message)
                        } else if let Some(context) = context {
                            context.send_oneway(message)
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
                            Err(_) => return Err(errno::EAGAIN),
                        }
                        return Ok(());
                    }
                    let call = if let Some(capability) = capability {
                        capability.send(message)
                    } else if let Some(context) = context {
                        context.send(message)
                    } else {
                        Err(crate::ipc::rpc::RpcError::PeerClosed)
                    };
                    match call {
                        Ok(call) => {
                            call.notify_on_completion(&self.endpoint.waker());
                            thread.outgoing = Some(call);
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
                        Err(_) => return Err(errno::EAGAIN),
                    }
                } else {
                    let request = {
                        let mut state = self.state.lock();
                        let thread = state.threads.get_mut(&tid).unwrap();
                        if thread.completed.len() >= 64 {
                            return Err(errno::EAGAIN);
                        }
                        let request = thread.incoming.take().ok_or(errno::EINVAL)?;
                        request
                    };
                    let _ = request.reply(message);
                    self.state
                        .lock()
                        .threads
                        .get_mut(&tid)
                        .unwrap()
                        .completed
                        .push_back(Completed {
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
            for thread in state.threads.values_mut() {
                thread.flush_generation = thread.flush_generation.wrapping_add(1);
            }
        }
        self.endpoint.waker().wake_all();
    }
    fn ready(&self, tid: usize) -> bool {
        let state = self.state.lock();
        let Some(thread) = state.threads.get(&tid) else {
            return false;
        };
        if thread.flush_generation != thread.returned_flush {
            return true;
        }
        let objects = self.objects.lock();
        let process_work = thread.looper && thread.outgoing.is_none() && thread.incoming.is_none();
        if objects.deaths.ready(tid, process_work) {
            return true;
        }
        if objects.ready(tid) {
            return true;
        }
        if objects.pending_acquisition(tid) {
            return false;
        }
        if let Some(completed) = thread.completed.front() {
            return completed.ready();
        }
        !objects.pending_local_acquisitions()
            && (thread.outgoing.as_ref().is_some_and(RpcCall::ready)
                || (thread.looper
                    && thread.outgoing.is_none()
                    && thread.incoming.is_none()
                    && self.endpoint.ready()))
    }
    fn waiter(&self, tid: usize) -> Arc<Waker> {
        // Incoming calls, object-reference work and outgoing completions all
        // notify this process source; readiness remains specific to each thread.
        let _ = tid;
        self.endpoint.waker()
    }
    fn thread_exit(&self, tid: usize) {
        let removed = self.state.lock().threads.remove(&tid);
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
        let descriptors=self.auto_fds.lock().remove(&address);
        drop(descriptors);
        // Release native serialization and budget only after userspace has
        // returned the receive buffer; other loopers can then dispatch it.
        drop(request);
        Ok(())
    }

    fn read_record(&self, task: &Task, output: usize, capacity: usize) -> Result<usize, usize> {
        self.read_record_for_abi(task,None,output,capacity)
    }
    fn read_record_for_abi(&self,task:&Task,abi:Option<&LinuxAbi>,output:usize,capacity:usize)->Result<usize,usize> {
        if self.domain.device==1 && task.deadline_enabled() {return Err(errno::EOPNOTSUPP);}
        let mut state = self.state.lock();
        let tid = task.get_id();
        let thread = state.threads.get(&tid).unwrap();
        if thread.flush_generation != thread.returned_flush {
            if capacity < 4 {
                return Err(errno::ENOSPC);
            }
            let generation = thread.flush_generation;
            drop(state);
            write(task, output, &BR_NOOP.to_le_bytes())?;
            // Usercopy may fault or race another close. A failed copy does not
            // consume the notification, and a newer generation stays pending.
            self.state
                .lock()
                .threads
                .get_mut(&tid)
                .unwrap()
                .returned_flush = generation;
            return Ok(4);
        }
        let process_work = thread.looper && thread.outgoing.is_none() && thread.incoming.is_none();
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
        if self.objects.lock().ready(tid) {
            if capacity < 20 {
                return Err(errno::ENOSPC);
            }
            drop(state);
            let record = {
                // Do not expose acquisition work from an export until its
                // transport admission has committed. A failed send must drop
                // the corresponding lease before another thread observes it.
                let _sending = self.send_gate.lock();
                self.objects.lock().reference_record(tid)
            };
            return record.map_or(Ok(0), |record| record.write(task, output));
        }
        // Exporting code can drop its last local reference after completion.
        // Finish the driver's acquire handshakes before reporting completion.
        if self.objects.lock().pending_acquisition(tid) {
            return Ok(0);
        }
        if let Some(completed) = state.threads.get(&tid).unwrap().completed.front() {
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
            self.state
                .lock()
                .threads
                .get_mut(&tid)
                .unwrap()
                .completed
                .pop_front();
            return Ok(4);
        }
        // A target or a returned local object embeds userspace pointers. Hold
        // payload delivery until acquisition is acknowledged. Message leases
        // then preserve the relevant holds through decode and usercopy.
        if self.objects.lock().pending_local_acquisitions() {
            return Ok(0);
        }
        let thread = state.threads.get(&tid).unwrap();
        let replying = thread.outgoing.as_ref().is_some_and(RpcCall::ready);
        let receiving = thread.looper
            && thread.outgoing.is_none()
            && thread.incoming.is_none()
            && self.endpoint.ready();
        if !replying && !receiving {
            return Ok(0);
        }
        if capacity < 68 {
            return Err(errno::ENOSPC);
        }
        let arena = state.arena.clone().ok_or(errno::EINVAL)?;
        let address = arena.reserve()?;
        let mut incoming = None;
        let message = if replying {
            state
                .threads
                .get_mut(&tid)
                .unwrap()
                .outgoing
                .take()
                .unwrap()
                .take_reply()
                .unwrap()
        } else {
            match self.endpoint.receive() {
                Ok(Some(mut request)) => {
                    let message =
                        core::mem::replace(&mut request.message, objects::message_empty());
                    incoming = Some(request);
                    Ok(message)
                }
                _ => {
                    arena.free(address)?;
                    return Ok(0);
                }
            }
        };
        let mut node = state.node;
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
                    request.message.retained = Some(Arc::new(dispatch));
                }
            }
            let mut installed=abi.map(|abi|buffers::Installed::new(task,abi));
            let (length, offsets_length) = self.decode_received(&message, address, &arena,installed.as_mut())?;
            let mut record = [0u8; 68];
            record[..4]
                .copy_from_slice(&(if replying { BR_REPLY } else { BR_TRANSACTION }).to_le_bytes());
            let tx = &mut record[4..];
            if !replying {
                put64(tx, 0, node.0);
                put64(tx, 8, node.1);
            }
            tx[16..20].copy_from_slice(&message.opcode.to_le_bytes());
            tx[20..32].copy_from_slice(&message.bytes[..12]);
            put64(tx, 32, length);
            put64(tx, 40, offsets_length);
            put64(tx, 48, address);
            put64(tx, 56, address + ((length + 7) & !7));
            write(task, output, &record)?;
            if let Some(installed)=installed {
                let automatic=installed.commit();
                if !automatic.is_empty() {
                    let previous=self.auto_fds.lock().insert(address,automatic);
                    drop(previous);
                }
            }
            if let Some(request) = incoming.take() {
                if request.is_oneway() {
                    self.objects.lock().oneway.insert(address, request);
                } else {
                    let mut state=self.state.lock();let thread=state.threads.get_mut(&tid).unwrap();
                    thread.incoming_accepts_fds=u32_at(&message.bytes,0)&0x10!=0;
                    thread.incoming = Some(request);
                }
            }
            Ok(68)
        })();
        if result != Ok(68) {
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
                    BC_TRANSACTION_SG|BC_REPLY_SG=>72,
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
                let n = self.read_record_for_abi(task,Some(abi), output, read_size)?;
                if n != 0 {
                    put64(&mut bwr, 32, n);
                    return Ok(());
                }
                if nonblocking {
                    return Err(errno::EAGAIN);
                }
                let waker = self.waiter(task.get_id());
                if waker.wait_result_until_owned(task.get_id(), tf, || self.ready(task.get_id()))
                    == WaitResult::Interrupted
                {
                    return Err(errno::EINTR);
                }
            }
        })();
        put64(&mut bwr, 8, consumed);
        write(task, arg, &bwr)?;
        result
    }
}

pub(super) fn task_exit(task: &Task) {
    for handle in task.handle_table.active_handles() {
        if let Some(object) = task.handle_table.get(handle) {
            if let Some(file) = object
                .as_file()
                .and_then(|file| file.as_any().downcast_ref::<BinderFile>())
            {
                if file.owner == task.get_thread_group_id() {
                    file.thread_exit(task.get_id());
                }
            }
        }
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
                device_id: 0,
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
                threads: BTreeMap::new(),
                arena: None,
                node: (0, 0),
            }),
        }
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
        assert!(server.mmap(&receiver, 0, 128 * 1024, 1, 2, 0) < usize::MAX - 4096);
        let output = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        receiver.allocate_stack_pages(output, 1).unwrap();
        // An oneway callback is allowed while this thread handles a sync call.
        let call = client.endpoint.send(objects::message_empty()).unwrap();
        client.state.lock().threads.get_mut(&1).unwrap().incoming =
            client.endpoint.receive().unwrap();
        let mut data = [0u8; 64];
        data[20..24].copy_from_slice(&1u32.to_le_bytes());
        client
            .command(&sender, &abi, BC_TRANSACTION, &data)
            .unwrap();
        assert!(client.state.lock().threads[&1].outgoing.is_none());
        assert!(client.state.lock().threads[&1].incoming.is_some());
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
            assert!(server.state.lock().threads[&2].incoming.is_none());
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
            assert!(file.mmap(task, 0, 128 * 1024, 1, 2, 0) < usize::MAX - 4096);
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
        assert_eq!(u32_at(&record, 24), FLAG_COLLECT_NOTED_APP_OPS|0x10);
        assert_eq!(u32_at(&record, 28), 1);
        server.command(&receiver, &abi, BC_REPLY, &data).unwrap();
        assert_eq!(client.read_record(&sender, output, 68), Ok(4));
        assert_eq!(u32_at(&read::<4>(&sender, output).unwrap(), 0), BR_COMPLETE);
        assert_eq!(client.read_record(&sender, output, 68), Ok(68));
        let record = read::<68>(&sender, output).unwrap();
        assert_eq!(u32_at(&record, 0), BR_REPLY);
        assert_eq!(u32_at(&record, 24), FLAG_COLLECT_NOTED_APP_OPS|0x10);
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
        put64(&mut data, 4, 0x1234_5678_8765_4321);
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
