//! Initial 64-bit Binder adapter over native RPC objects. Supports bounded,
//! synchronous, byte-only transactions to the context manager. Object/FD
//! transfer, one-way calls and nested transactions are explicitly unsupported.

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
use arena::Arena;

const WRITE_READ: u32 = 0xc0306201;
const VERSION: u32 = 0xc0046209;
const SET_CONTEXT: u32 = 0x40046207;
const SET_CONTEXT_EXT: u32 = 0x4018620d;
const SET_MAX_THREADS: u32 = 0x40046205;
const THREAD_EXIT: u32 = 0x40046208;
const BC_TRANSACTION: u32 = 0x40406300;
const BC_REPLY: u32 = 0x40406301;
const BC_FREE_BUFFER: u32 = 0x40086303;
const BC_REGISTER_LOOPER: u32 = 0x630b;
const BC_ENTER_LOOPER: u32 = 0x630c;
const BC_EXIT_LOOPER: u32 = 0x630d;
const BR_TRANSACTION: u32 = 0x80407202;
const BR_REPLY: u32 = 0x80407203;
const BR_DEAD_REPLY: u32 = 0x7205;
const BR_COMPLETE: u32 = 0x7206;
const MAX_THREADS: usize = 64;
const MAX_DATA: usize = crate::ipc::rpc::MAX_MESSAGE_SIZE - 12;

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

struct Context {
    endpoint: Weak<RpcEndpoint>,
}
struct Domain {
    // Retain the environment so pointer identity cannot be reused while open.
    environment: Arc<Environment>,
    context: IrqSpinLock<Option<Context>>,
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
        context: IrqSpinLock::new(None),
    });
    domains.insert(key, Arc::downgrade(&domain));
    domain
}

#[derive(Default)]
struct Thread {
    looper: bool,
    completed: VecDeque<u32>,
    outgoing: Option<RpcCall>,
    incoming: Option<RpcRequest>,
}
struct State {
    threads: BTreeMap<usize, Thread>,
    arena: Option<Arc<Arena>>,
    node: (usize, usize),
}
pub(crate) struct BinderFile {
    owner: usize,
    domain: Arc<Domain>,
    endpoint: Arc<RpcEndpoint>,
    state: IrqSpinLock<State>,
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
            domain: domain(environment, device),
            endpoint: RpcEndpoint::new(),
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
                    });
                    Ok(())
                }
                THREAD_EXIT => {
                    self.state.lock().threads.remove(&task.get_id());
                    Ok(())
                }
                WRITE_READ => self.write_read(task, abi, tf, arg, nonblocking),
                _ => Err(errno::ENOTTY),
            }
        })();
        result.map_or_else(errno::to_result, |_| 0)
    }
    fn command(&self, task: &Task, abi: &LinuxAbi, command: u32, data: &[u8]) -> Result<(), usize> {
        let tid = task.get_id();
        match command {
            BC_ENTER_LOOPER | BC_REGISTER_LOOPER | BC_EXIT_LOOPER => {
                self.state.lock().threads.get_mut(&tid).unwrap().looper = command != BC_EXIT_LOOPER;
            }
            BC_FREE_BUFFER => {
                self.state
                    .lock()
                    .arena
                    .as_ref()
                    .ok_or(errno::EINVAL)?
                    .free(u64_at(data, 0))?;
            }
            BC_TRANSACTION | BC_REPLY => {
                let flags = u32_at(data, 20);
                let len = u64_at(data, 32);
                if u64_at(data, 40) != 0
                    || flags & !(0x10 | 0x20 | if command == BC_REPLY { 8 } else { 0 }) != 0
                {
                    return Err(errno::EOPNOTSUPP);
                }
                if len > MAX_DATA {
                    return Err(errno::E2BIG);
                }
                let mut payload = vec![0; len + 12];
                payload[..4].copy_from_slice(&(flags & 8).to_le_bytes());
                payload[4..8]
                    .copy_from_slice(&(abi.visible_thread_group_id(task) as u32).to_le_bytes());
                // Scarlet's Linux credential model currently reports euid 0.
                copy_from_user(task, u64_at(data, 48), &mut payload[12..])
                    .map_err(|_| errno::EFAULT)?;
                let message = RpcMessage {
                    opcode: u32_at(data, 16),
                    bytes: payload,
                };
                if command == BC_TRANSACTION {
                    if u64_at(data, 0) != 0 {
                        return Err(errno::EOPNOTSUPP);
                    }
                    let target = self
                        .domain
                        .context
                        .lock()
                        .as_ref()
                        .and_then(|c| c.endpoint.upgrade());
                    let mut state = self.state.lock();
                    let thread = state.threads.get_mut(&tid).unwrap();
                    if thread.outgoing.is_some() || thread.incoming.is_some() {
                        return Err(errno::EOPNOTSUPP);
                    }
                    if thread.completed.len() >= 64 {
                        return Err(errno::EAGAIN);
                    }
                    if let Some(target) = target {
                        thread.outgoing = Some(target.send(message).map_err(|_| errno::EAGAIN)?);
                        thread.completed.push_back(BR_COMPLETE);
                    } else {
                        thread.completed.push_back(BR_DEAD_REPLY);
                    }
                } else {
                    let request = {
                        let mut state = self.state.lock();
                        let thread = state.threads.get_mut(&tid).unwrap();
                        if thread.completed.len() >= 64 {
                            return Err(errno::EAGAIN);
                        }
                        let request = thread.incoming.take().ok_or(errno::EINVAL)?;
                        thread.completed.push_back(BR_COMPLETE);
                        request
                    };
                    // A caller may have died while the server was processing.
                    let _ = request.reply(message);
                }
            }
            _ => return Err(errno::EOPNOTSUPP),
        }
        Ok(())
    }
    fn ready(&self, tid: usize) -> bool {
        let state = self.state.lock();
        let Some(thread) = state.threads.get(&tid) else {
            return false;
        };
        !thread.completed.is_empty()
            || thread.outgoing.as_ref().is_some_and(RpcCall::ready)
            || (thread.looper
                && thread.outgoing.is_none()
                && thread.incoming.is_none()
                && self.endpoint.ready())
    }
    fn waiter(&self, tid: usize) -> Arc<Waker> {
        self.state
            .lock()
            .threads
            .get(&tid)
            .and_then(|t| t.outgoing.as_ref().map(RpcCall::waker))
            .unwrap_or_else(|| self.endpoint.waker())
    }
    fn read_record(&self, task: &Task, output: usize, capacity: usize) -> Result<usize, usize> {
        let mut state = self.state.lock();
        let tid = task.get_id();
        if let Some(command) = state.threads.get(&tid).unwrap().completed.front().copied() {
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
                Ok(Some(request)) => {
                    let message = RpcMessage {
                        opcode: request.message.opcode,
                        bytes: request.message.bytes.clone(),
                    };
                    incoming = Some(request);
                    Ok(message)
                }
                _ => {
                    arena.free(address)?;
                    return Ok(0);
                }
            }
        };
        let node = state.node;
        drop(state);
        let result = (|| {
            let message = match message {
                Ok(message) => message,
                Err(_) => {
                    write(task, output, &BR_DEAD_REPLY.to_le_bytes())?;
                    return Ok(4);
                }
            };
            let payload = &message.bytes[12..];
            arena.store(address, payload)?;
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
            put64(tx, 32, payload.len());
            put64(tx, 48, address);
            put64(tx, 56, address + ((payload.len() + 7) & !7));
            write(task, output, &record)?;
            if let Some(request) = incoming.take() {
                self.state.lock().threads.get_mut(&tid).unwrap().incoming = Some(request);
            }
            Ok(68)
        })();
        if result != Ok(68) {
            let _ = arena.free(address);
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
                    BC_FREE_BUFFER => 8,
                    BC_ENTER_LOOPER | BC_REGISTER_LOOPER | BC_EXIT_LOOPER => 0,
                    _ => return Err(errno::EOPNOTSUPP),
                };
                if write_size - consumed < 4 + size {
                    return Err(errno::EINVAL);
                }
                let mut data = [0; 64];
                copy_from_user(task, source + consumed + 4, &mut data[..size])
                    .map_err(|_| errno::EFAULT)?;
                self.command(task, abi, command, &data[..size])?;
                consumed += 4 + size;
            }
            if read_size == 0 {
                return Ok(());
            }
            loop {
                let n = self.read_record(task, output, read_size)?;
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
        _: ReadyInterest,
        _: &mut Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        // Poll-based loopers are not yet supported; ioctl uses the RPC wakers.
        SelectWaitOutcome::Interrupted
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
            domain: domain(environment.clone(), device),
            endpoint: RpcEndpoint::new(),
            state: IrqSpinLock::new(State {
                threads: BTreeMap::new(),
                arena: None,
                node: (0, 0),
            }),
        }
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
                    .copied(),
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
