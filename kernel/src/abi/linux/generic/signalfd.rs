//! Linux signal records over Scarlet's stream/readiness capabilities.
//! The descriptor stores only its mask: reads resolve the current task's state,
//! so a descriptor inherited across exec or passed to another task cannot drain
//! the creator's pending signals.

use super::{
    LinuxAbi, errno,
    signal::{LinuxSignal, SignalInfo, SignalState},
};
use crate::{
    arch::Trapframe,
    fs::{FileMetadata, SeekFrom},
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingOps, ReadyInterest, ReadySet, SelectWaitOutcome,
            Selectable, StreamError, StreamOps,
        },
    },
    sync::{IrqSpinLock, waker::WaitResult},
    task::mytask,
};
use alloc::{sync::Arc,vec,vec::Vec};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, Ordering},
};

const NONBLOCK: u32 = 0x800;
const CLOEXEC: u32 = 0x80000;
const UNMASKABLE: u64 = (1 << (9 - 1)) | (1 << (19 - 1));
const RECORD_SIZE: usize = 128;

pub(super) struct SignalFd {
    mask: IrqSpinLock<u64>,
    nonblocking: AtomicBool,
}

fn reader_state() -> Option<Arc<IrqSpinLock<SignalState>>> {
    mytask()?.linux_signal_state.lock().as_ref()?.upgrade()
}

impl SignalFd {
    fn new(mask: u64, nonblocking: bool) -> Self {
        Self {
            mask: IrqSpinLock::new(mask & !UNMASKABLE),
            nonblocking: AtomicBool::new(nonblocking),
        }
    }

    fn ready(&self, state: &SignalState) -> bool {
        state.pending_mask() & *self.mask.lock() != 0
    }

    fn read_pending(&self, state: &mut SignalState, buffer: &mut [u8]) -> usize {
        let mut count = 0;
        for record in buffer.chunks_exact_mut(RECORD_SIZE) {
            let Some((signal, info)) = state.take_pending_matching(*self.mask.lock()) else {
                break;
            };
            write_record(record, signal, &info);
            count += RECORD_SIZE;
        }
        count
    }
}

fn write_record(record: &mut [u8], signal: LinuxSignal, info: &SignalInfo) {
    record.fill(0);
    record[..4].copy_from_slice(&(signal as u32).to_ne_bytes());
    // SignalInfo::kernel supplies SI_KERNEL for records without payload data.
    record[4..8].copy_from_slice(&info.bytes[4..8]);
    record[8..12].copy_from_slice(&info.bytes[8..12]);
    record[12..16].copy_from_slice(&info.bytes[16..20]);
    record[16..20].copy_from_slice(&info.bytes[20..24]);
    record[44..48].copy_from_slice(&info.bytes[24..28]);
    record[48..56].copy_from_slice(&info.bytes[24..32]);
}

impl StreamOps for SignalFd {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if buffer.len() < RECORD_SIZE {
            return Err(StreamError::InvalidArgument);
        }
        let state = reader_state().ok_or(StreamError::NotSupported)?;
        let waker = state.lock().pending_waker.clone();
        loop {
            let count = self.read_pending(&mut state.lock(), buffer);
            if count != 0 {
                return Ok(count);
            }
            if self.is_nonblocking() {
                return Err(StreamError::WouldBlock);
            }
            let task = mytask().ok_or(StreamError::Interrupted)?;
            if waker.wait_with_condition(task.get_id(), task.get_trapframe(), None, 0, || {
                self.ready(&state.lock())
            }) == WaitResult::Interrupted
            {
                return Err(StreamError::Interrupted);
            }
        }
    }
    fn write(&self, _: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::InvalidArgument)
    }
}

impl Selectable for SignalFd {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut ready = ReadySet::none();
        ready.read = interest.read && reader_state().is_some_and(|s| self.ready(&s.lock()));
        ready
    }
    fn subscribe_readiness(
        &self,
        interest: ReadyInterest,
        waiter: &Arc<crate::sync::Waker>,
    ) -> Option<Vec<crate::sync::readiness::ReadinessRegistration>> {
        if !interest.read {
            return Some(Vec::new());
        }
        let state = reader_state()?;
        let waker = state.lock().pending_waker.clone();
        Some(vec![waker.subscribe_readiness(waiter)])
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        tf: &mut Trapframe,
        timeout: Option<u64>,
        minimum: u64,
    ) -> SelectWaitOutcome {
        let Some(state) = reader_state() else {
            return SelectWaitOutcome::TimedOut;
        };
        let Some(task) = mytask() else {
            return SelectWaitOutcome::TimedOut;
        };
        let waker = state.lock().pending_waker.clone();
        match waker.wait_with_condition(task.get_id(), tf, timeout, minimum, || {
            interest.read && self.ready(&state.lock())
        }) {
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            _ => SelectWaitOutcome::Ready,
        }
    }
    fn set_nonblocking(&self, enabled: bool) {
        self.nonblocking.store(enabled, Ordering::Release);
    }
    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }
}
impl ControlOps for SignalFd {}
impl MemoryMappingOps for SignalFd {
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("signalfd does not support mmap")
    }
}
impl FileObject for SignalFd {
    fn seek(&self, _: SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::NotSupported)
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Err(StreamError::NotSupported)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn sys_signalfd4(abi: &mut LinuxAbi, tf: &mut Trapframe) -> usize {
    let Some(task) = mytask() else {
        return errno::to_result(errno::ESRCH);
    };
    let fd = tf.get_arg(0) as i32;
    let mask_ptr = tf.get_arg(1);
    let size = tf.get_arg(2);
    let flags = tf.get_arg(3) as u32;
    tf.increment_pc_next(&task);
    if size != 8 || flags & !(NONBLOCK | CLOEXEC) != 0 {
        return errno::to_result(errno::EINVAL);
    }
    let mut bytes = [0; 8];
    if crate::library::std::usercopy::copy_from_user(&task, mask_ptr, &mut bytes).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let mask = u64::from_ne_bytes(bytes) & !UNMASKABLE;
    abi.bind_task_signals(&task);
    if fd != -1 {
        let Some(handle) = abi.get_handle(fd as usize) else {
            return errno::to_result(errno::EBADF);
        };
        let Some(object) = task.handle_table.get_arc_clone(handle) else {
            return errno::to_result(errno::EBADF);
        };
        let Some(file) = object
            .as_file()
            .and_then(|f| f.as_any().downcast_ref::<SignalFd>())
        else {
            return errno::to_result(errno::EINVAL);
        };
        *file.mask.lock() = mask;
        // Updating a mask does not change the existing descriptor's flags.
        let waker = abi.signal_state.lock().pending_waker.clone();
        waker.wake_all();
        return fd as usize;
    }
    let object = KernelObject::File(Arc::new(SignalFd::new(mask, flags & NONBLOCK != 0)));
    let Ok(handle) = task.handle_table.insert(object) else {
        return errno::to_result(errno::EMFILE);
    };
    let Ok(fd) = abi.allocate_fd(handle) else {
        let _ = task.handle_table.remove(handle);
        return errno::to_result(errno::EMFILE);
    };
    let _ = abi.set_file_status_flags(fd, flags & NONBLOCK);
    if flags & CLOEXEC != 0 {
        let _ = abi.set_fd_flags(fd, super::fs::FD_CLOEXEC);
    }
    fd
}

#[cfg(test)]
mod tests {
    use super::super::signal::LinuxSignal;
    use super::*;
    use crate::task::{Task, TaskType, clear_mock_current_task, set_mock_current_task};

    #[test_case]
    fn signalfd_syscalls_update_duplicates_and_read_across_user_pages() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        use crate::library::std::usercopy::{copy_from_user, copy_to_user};
        let task = Arc::new(Task::new("signalfd-syscall".into(), 1, TaskType::User));
        task.allocate_stack_pages(USER_STACK_END - PAGE_SIZE * 2, 2)
            .unwrap();
        set_mock_current_task(task.clone());
        let mut abi = LinuxAbi::default();
        let mask_address = USER_STACK_END - 16;
        copy_to_user(&task, mask_address, &(1u64 << 9).to_ne_bytes()).unwrap();
        let mut tf = Trapframe::new();
        let code = USER_STACK_END - PAGE_SIZE * 2;
        for index in 0..8 {
            copy_to_user(&task, code + index * 4, &0x73u32.to_ne_bytes()).unwrap();
        }
        tf.set_pc(code as u64);
        tf.set_arg(0, usize::MAX);
        tf.set_arg(1, mask_address);
        tf.set_arg(2, 8);
        tf.set_arg(3, (NONBLOCK | CLOEXEC) as usize);
        let fd = sys_signalfd4(&mut abi, &mut tf);
        assert_eq!(abi.get_fd_flags(fd), Some(super::super::fs::FD_CLOEXEC));
        tf.set_arg(0, fd);
        tf.set_arg(1, 0); // F_DUPFD
        tf.set_arg(2, 10);
        let duplicate = super::super::fs::sys_fcntl(&mut abi, &mut tf);
        assert_eq!(duplicate, 10);
        // Updating a duplicated descriptor must preserve the mask's high bits
        // even when the target has no native 64-bit atomics.
        copy_to_user(&task, mask_address, &(1u64 << 63).to_ne_bytes()).unwrap();
        tf.set_arg(0, duplicate);
        tf.set_arg(1, mask_address);
        tf.set_arg(2, 8);
        tf.set_arg(3, 0);
        assert_eq!(sys_signalfd4(&mut abi, &mut tf), duplicate);
        abi.signal_state.lock().add_pending(LinuxSignal::SIGUSR1);
        abi.signal_state.lock().add_pending(LinuxSignal::SIGRT32);
        let buffer = USER_STACK_END - PAGE_SIZE - 7;
        copy_to_user(&task, buffer, &[0x55; 129]).unwrap();
        tf.set_arg(0, fd);
        tf.set_arg(1, buffer);
        tf.set_arg(2, 128);
        assert_eq!(super::super::fs::sys_read(&mut abi, &mut tf), 128);
        let mut record = [0; 129];
        copy_from_user(&task, buffer, &mut record).unwrap();
        assert_eq!(u32::from_ne_bytes(record[..4].try_into().unwrap()), 64);
        assert_eq!(record[128], 0x55);
        assert!(abi.signal_state.lock().is_pending(LinuxSignal::SIGUSR1));
        assert_eq!(
            super::super::fs::sys_read(&mut abi, &mut tf),
            errno::to_result(errno::EAGAIN)
        );
        clear_mock_current_task();
    }

    #[test_case]
    fn signalfd_consumes_records_and_resolves_the_reader_after_fork_and_exec() {
        let task = Arc::new(Task::new("signalfd".into(), 1, TaskType::Kernel));
        set_mock_current_task(task.clone());
        let abi = LinuxAbi::default();
        abi.bind_task_signals(&task);
        let fd = SignalFd::new(u64::MAX, true);
        let mut records = [0x55; 257];
        assert!(matches!(
            fd.read(&mut records),
            Err(StreamError::WouldBlock)
        ));
        abi.signal_state.lock().add_pending(LinuxSignal::SIGUSR1);
        abi.signal_state.lock().add_pending(LinuxSignal::SIGRT32);
        abi.signal_state.lock().add_pending(LinuxSignal::SIGKILL);
        assert!(fd.current_ready(ReadyInterest::read()).read);
        assert!(matches!(
            fd.read(&mut records[..127]),
            Err(StreamError::InvalidArgument)
        ));
        assert_eq!(fd.read(&mut records).unwrap(), 256);
        assert_eq!(u32::from_ne_bytes(records[..4].try_into().unwrap()), 10);
        assert_eq!(
            u32::from_ne_bytes(records[128..132].try_into().unwrap()),
            64
        );
        assert_eq!(records[256], 0x55);
        assert!(abi.signal_state.lock().is_pending(LinuxSignal::SIGKILL));
        assert!(!fd.current_ready(ReadyInterest::read()).read);
        abi.signal_state.lock().add_pending(LinuxSignal::SIGUSR1);
        let mut next = abi.clone();
        next.fork_signal_state();
        next.bind_task_signals(&task);
        assert!(matches!(
            fd.read(&mut records),
            Err(StreamError::WouldBlock)
        ));
        assert!(abi.signal_state.lock().is_pending(LinuxSignal::SIGUSR1));
        next.signal_state.lock().add_pending(LinuxSignal::SIGCHLD);
        let mut exec = LinuxAbi::default();
        exec.prepare_exec_fds(Some(&next), &task);
        exec.bind_task_signals(&task);
        assert_eq!(fd.read(&mut records).unwrap(), 128);
        assert_eq!(u32::from_ne_bytes(records[..4].try_into().unwrap()), 17);
        clear_mock_current_task();
    }

    #[test_case]
    fn signalfd_maps_sender_and_sigval_from_signalfd_siginfo() {
        let mut info = SignalInfo::kernel(LinuxSignal::SIGRT32);
        info.bytes[8..12].copy_from_slice(&(-1i32).to_ne_bytes());
        info.bytes[16..20].copy_from_slice(&1234i32.to_ne_bytes());
        info.bytes[20..24].copy_from_slice(&5678u32.to_ne_bytes());
        info.bytes[24..32].copy_from_slice(&0x1234_5678_9abc_def0u64.to_ne_bytes());
        let mut record = [0xa5; RECORD_SIZE];
        write_record(&mut record, LinuxSignal::SIGRT32, &info);
        assert_eq!(u32::from_ne_bytes(record[..4].try_into().unwrap()), 64);
        assert_eq!(i32::from_ne_bytes(record[8..12].try_into().unwrap()), -1);
        assert_eq!(u32::from_ne_bytes(record[12..16].try_into().unwrap()), 1234);
        assert_eq!(u32::from_ne_bytes(record[16..20].try_into().unwrap()), 5678);
        assert_eq!(
            u32::from_ne_bytes(record[44..48].try_into().unwrap()),
            0x9abc_def0
        );
        assert_eq!(
            u64::from_ne_bytes(record[48..56].try_into().unwrap()),
            0x1234_5678_9abc_def0
        );
    }

    #[test_case]
    fn signalfd_consumes_one_record_at_a_time_for_repeated_realtime_signals() {
        let task = Arc::new(Task::new("signalfd-realtime".into(), 1, TaskType::Kernel));
        set_mock_current_task(task.clone());
        let abi = LinuxAbi::default();
        abi.bind_task_signals(&task);
        let fd = SignalFd::new(u64::MAX, true);
        for payload in [0x1111u64, 0x2222] {
            let mut info = SignalInfo::kernel(LinuxSignal::SIGRT32);
            info.bytes[24..32].copy_from_slice(&payload.to_ne_bytes());
            abi.signal_state
                .lock()
                .queue_pending_info(LinuxSignal::SIGRT32, info)
                .unwrap();
        }
        let mut record = [0; RECORD_SIZE];
        assert_eq!(fd.read(&mut record).unwrap(), RECORD_SIZE);
        assert_eq!(u32::from_ne_bytes(record[..4].try_into().unwrap()), 64);
        assert_eq!(
            u64::from_ne_bytes(record[48..56].try_into().unwrap()),
            0x1111
        );
        assert!(abi.signal_state.lock().is_pending(LinuxSignal::SIGRT32));
        assert_eq!(fd.read(&mut record).unwrap(), RECORD_SIZE);
        assert_eq!(
            u64::from_ne_bytes(record[48..56].try_into().unwrap()),
            0x2222
        );
        assert!(!abi.signal_state.lock().is_pending(LinuxSignal::SIGRT32));
        clear_mock_current_task();
    }
}
