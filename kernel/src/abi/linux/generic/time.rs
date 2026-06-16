//! Time-related system calls for Linux ABI
//!
//! This module implements Linux time system calls for the Scarlet kernel,
//! providing compatibility with Linux userspace programs that need time information.

use super::{
    errno,
    signal::{LinuxSignal, SignalState},
};
use crate::{
    abi::linux::generic::LinuxAbi,
    arch::Trapframe,
    object::KernelObject,
    object::timer::Timer,
    sched::scheduler::wake_task,
    task::mytask,
    time::{current_time, current_time_ns},
    timer::{TimerHandler, add_timer, cancel_timer, get_tick, ns_to_ticks, ticks_to_ns},
};
use alloc::sync::{Arc, Weak};
use spin::Mutex;

const NSEC_PER_SEC_I64: i64 = 1_000_000_000;
const NSEC_PER_SEC_U64: u64 = 1_000_000_000;
const TIMER_ABSTIME: i32 = 1;

/// Linux POSIX timer shared state guarded by a spin mutex for interior mutability.
pub struct PosixTimerShared {
    state: Mutex<PosixTimerState>,
}

impl PosixTimerShared {
    fn new(
        id: u64,
        clock_id: i32,
        sigev_notify: i32,
        sigev_signo: i32,
        sigev_value: u64,
        owner_task_id: usize,
        signal_state: Arc<spin::Mutex<SignalState>>,
    ) -> Self {
        Self {
            state: Mutex::new(PosixTimerState {
                id,
                clock_id,
                sigev_notify,
                sigev_signo,
                sigev_value,
                interval_ns: 0,
                timer_entry_id: None,
                next_deadline_tick: None,
                active: false,
                owner_task_id,
                signal_state,
                overrun_count: 0,
            }),
        }
    }

    fn lock(&self) -> spin::MutexGuard<'_, PosixTimerState> {
        self.state.lock()
    }
}

/// Mutable state stored for each POSIX timer.
pub struct PosixTimerState {
    pub id: u64,
    pub clock_id: i32,
    pub sigev_notify: i32,
    pub sigev_signo: i32,
    pub sigev_value: u64,
    pub interval_ns: u64,
    pub timer_entry_id: Option<u64>,
    pub next_deadline_tick: Option<u64>,
    pub active: bool,
    pub owner_task_id: usize,
    pub signal_state: Arc<spin::Mutex<SignalState>>,
    pub overrun_count: u32,
}

/// Public representation of a POSIX timer stored in the Linux ABI state.
#[derive(Clone)]
pub struct PosixTimer {
    pub id: u64,
    shared: Arc<PosixTimerShared>,
    handler: Arc<PosixTimerHandler>,
}

impl PosixTimer {
    pub fn new(
        id: u64,
        clock_id: i32,
        sigev_notify: i32,
        sigev_signo: i32,
        sigev_value: u64,
        owner_task_id: usize,
        signal_state: Arc<spin::Mutex<SignalState>>,
    ) -> Self {
        let shared = Arc::new(PosixTimerShared::new(
            id,
            clock_id,
            sigev_notify,
            sigev_signo,
            sigev_value,
            owner_task_id,
            signal_state,
        ));
        let handler = Arc::new(PosixTimerHandler {
            timer_id: id,
            shared: Arc::downgrade(&shared),
        });
        Self {
            id,
            shared,
            handler,
        }
    }

    /// Schedule (or reschedule) this timer with the specified first expiration and interval.
    ///
    /// A zero `first_ns` disarms the timer.
    pub fn schedule(&self, first_ns: u64, interval_ns: u64) {
        let mut state = self.shared.lock();
        if let Some(entry_id) = state.timer_entry_id.take() {
            cancel_timer(entry_id);
        }
        state.interval_ns = interval_ns;
        state.overrun_count = 0;

        if first_ns == 0 {
            state.active = false;
            state.next_deadline_tick = None;
            return;
        }

        let mut ticks = ns_to_ticks(first_ns);
        if ticks == 0 {
            ticks = 1;
        }
        let target_tick = get_tick().saturating_add(ticks);
        let handler_dyn: Arc<dyn TimerHandler> = self.handler.clone();
        let new_id = add_timer(target_tick, &handler_dyn, self.id as usize);
        state.timer_entry_id = Some(new_id);
        state.next_deadline_tick = Some(target_tick);
        state.active = true;
    }

    /// Cancel any outstanding kernel timer and mark this POSIX timer inactive.
    pub fn cancel(&self) {
        let mut state = self.shared.lock();
        if let Some(entry_id) = state.timer_entry_id.take() {
            cancel_timer(entry_id);
        }
        state.active = false;
        state.next_deadline_tick = None;
    }

    /// Snapshot the remaining time (in ns) and current interval (in ns).
    pub fn snapshot(&self) -> (u64, u64) {
        let state = self.shared.lock();
        let interval = state.interval_ns;
        let remaining = match state.next_deadline_tick {
            Some(deadline) => {
                let now = get_tick();
                if deadline > now {
                    ticks_to_ns(deadline - now)
                } else {
                    0
                }
            }
            None => 0,
        };
        (remaining, interval)
    }

    pub fn state(&self) -> spin::MutexGuard<'_, PosixTimerState> {
        self.shared.lock()
    }
}

struct PosixTimerHandler {
    timer_id: u64,
    shared: Weak<PosixTimerShared>,
}

impl TimerHandler for PosixTimerHandler {
    fn on_timer_expired(self: Arc<Self>, _context: usize) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };

        // Snapshot data while holding the state lock
        let (notify, signo, interval_ns, owner_task_id, signal_state) = {
            let mut state = shared.lock();
            if !state.active {
                return;
            }
            state.timer_entry_id = None;
            state.next_deadline_tick = None;
            (
                state.sigev_notify,
                state.sigev_signo,
                state.interval_ns,
                state.owner_task_id,
                state.signal_state.clone(),
            )
        };

        // Deliver notifications outside of the lock
        if notify == SIGEV_SIGNAL {
            if let Some(signal) = LinuxSignal::from_u32(signo as u32) {
                let mut locked = signal_state.lock();
                locked.add_pending(signal);
                drop(locked);
                let _ = wake_task(owner_task_id);
            }
        }

        if notify == SIGEV_NONE {
            // No wake required for SIGEV_NONE.
        }

        // Re-arm periodic timers.
        if interval_ns > 0 {
            let mut state = shared.lock();
            let mut ticks = ns_to_ticks(interval_ns);
            if ticks == 0 {
                ticks = 1;
            }
            let target_tick = get_tick().saturating_add(ticks);
            let handler_dyn: Arc<dyn TimerHandler> = self.clone();
            let entry_id = add_timer(target_tick, &handler_dyn, self.timer_id as usize);
            state.timer_entry_id = Some(entry_id);
            state.next_deadline_tick = Some(target_tick);
            state.active = true;
        } else {
            let mut state = shared.lock();
            state.active = false;
        }
    }
}

/// Linux sigevent notification values (subset).
pub const SIGEV_SIGNAL: i32 = 0;
pub const SIGEV_NONE: i32 = 1;
pub const SIGEV_THREAD: i32 = 2;
pub const SIGEV_THREAD_ID: i32 = 4;

fn is_supported_clock(clock_id: i32) -> bool {
    matches!(
        clock_id,
        CLOCK_REALTIME
            | CLOCK_MONOTONIC
            | CLOCK_PROCESS_CPUTIME_ID
            | CLOCK_THREAD_CPUTIME_ID
            | CLOCK_MONOTONIC_RAW
            | CLOCK_REALTIME_COARSE
            | CLOCK_MONOTONIC_COARSE
            | CLOCK_BOOTTIME
    )
}

fn timespec_to_ns(ts: &TimeSpec) -> Result<u64, usize> {
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= NSEC_PER_SEC_I64 {
        return Err(errno::EINVAL);
    }
    let sec = ts.tv_sec as u128;
    let nsec = ts.tv_nsec as u128;
    let total = sec
        .saturating_mul(NSEC_PER_SEC_U64 as u128)
        .saturating_add(nsec);
    Ok(total.min(u64::MAX as u128) as u64)
}

fn ns_to_timespec(ns: u64) -> TimeSpec {
    let sec = (ns / NSEC_PER_SEC_U64).min(i64::MAX as u64);
    let nsec = if sec >= i64::MAX as u64 {
        (NSEC_PER_SEC_U64 - 1) as i64
    } else {
        (ns % NSEC_PER_SEC_U64) as i64
    };
    TimeSpec {
        tv_sec: sec as i64,
        tv_nsec: nsec,
    }
}

/// Linux timespec structure (matches Linux userspace)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TimeSpec {
    pub tv_sec: i64,  // seconds
    pub tv_nsec: i64, // nanoseconds
}

/// Linux itimerspec structure used by timer_settime/gettime.
#[repr(C)]
#[derive(Clone, Copy)]
struct ItimerSpec {
    it_interval: TimeSpec,
    it_value: TimeSpec,
}

/// Linux clock IDs (subset of commonly used ones)
pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
pub const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
pub const CLOCK_MONOTONIC_RAW: i32 = 4;
pub const CLOCK_REALTIME_COARSE: i32 = 5;
pub const CLOCK_MONOTONIC_COARSE: i32 = 6;
pub const CLOCK_BOOTTIME: i32 = 7;

const TFD_NONBLOCK: i32 = 0o00004000;
const TFD_CLOEXEC: i32 = 0o02000000;
const TFD_TIMER_CANCEL_ON_SET: i32 = 1 << 1;

/// Linux `timerfd_create` implementation.
pub fn sys_timerfd_create(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return errno::to_result(errno::EFAULT),
    };

    let clock_id = trapframe.get_arg(0) as i32;
    let flags = trapframe.get_arg(1) as i32;

    trapframe.increment_pc_next(&task);

    if !is_supported_clock(clock_id) {
        return errno::to_result(errno::EINVAL);
    }

    let valid_flags = TFD_NONBLOCK | TFD_CLOEXEC;
    if (flags & !valid_flags) != 0 {
        return errno::to_result(errno::EINVAL);
    }

    let timer = Arc::new(Timer::new(clock_id, (flags & TFD_NONBLOCK) != 0));
    let kernel_obj = KernelObject::from_timer(timer);
    let handle = match task.handle_table.insert(kernel_obj) {
        Ok(handle) => handle,
        Err(_) => return errno::to_result(errno::EMFILE),
    };

    let linux_fd = match abi.allocate_fd(handle) {
        Ok(linux_fd) => linux_fd,
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            return errno::to_result(errno::EMFILE);
        }
    };

    if (flags & TFD_CLOEXEC) != 0 {
        let _ = abi.set_fd_flags(linux_fd, crate::abi::linux::generic::fs::FD_CLOEXEC);
    }
    if (flags & TFD_NONBLOCK) != 0 {
        let _ =
            abi.set_file_status_flags(linux_fd, crate::abi::linux::generic::fs::O_NONBLOCK as u32);
    }

    linux_fd
}

/// Linux `timerfd_settime` implementation.
pub fn sys_timerfd_settime(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return errno::to_result(errno::EFAULT),
    };

    let linux_fd = trapframe.get_arg(0);
    let flags = trapframe.get_arg(1) as i32;
    let new_value_ptr = trapframe.get_arg(2);
    let old_value_ptr = trapframe.get_arg(3);

    trapframe.increment_pc_next(&task);

    let valid_flags = TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET;
    if (flags & !valid_flags) != 0 {
        return errno::to_result(errno::EINVAL);
    }
    if new_value_ptr == 0 {
        return errno::to_result(errno::EFAULT);
    }

    let handle = match abi.get_handle(linux_fd) {
        Some(handle) => handle,
        None => return errno::to_result(errno::EBADF),
    };
    let kernel_obj = match task.handle_table.get(handle) {
        Some(kernel_obj) => kernel_obj,
        None => return errno::to_result(errno::EBADF),
    };
    let timer = match kernel_obj.as_timer() {
        Some(timer) => timer,
        None => return errno::to_result(errno::EINVAL),
    };

    let new_value_paddr = match task.vm_manager.translate_to_kva(new_value_ptr) {
        Some(addr) => addr,
        None => return errno::to_result(errno::EFAULT),
    };
    let new_spec = unsafe { *(new_value_paddr as *const ItimerSpec) };

    let first_ns = match timespec_to_ns(&new_spec.it_value) {
        Ok(ns) => ns,
        Err(errno_val) => return errno::to_result(errno_val),
    };
    let interval_ns = match timespec_to_ns(&new_spec.it_interval) {
        Ok(ns) => ns,
        Err(errno_val) => return errno::to_result(errno_val),
    };

    if old_value_ptr != 0 {
        let old_value_paddr = match task.vm_manager.translate_to_kva(old_value_ptr) {
            Some(addr) => addr as *mut ItimerSpec,
            None => return errno::to_result(errno::EFAULT),
        };
        let (remaining_ns, previous_interval_ns) = timer.snapshot();
        let snapshot = ItimerSpec {
            it_interval: ns_to_timespec(previous_interval_ns),
            it_value: ns_to_timespec(remaining_ns),
        };
        unsafe {
            *old_value_paddr = snapshot;
        }
    }

    timer.set_time(first_ns, interval_ns, (flags & TIMER_ABSTIME) != 0);
    0
}

/// Linux `timerfd_gettime` implementation.
pub fn sys_timerfd_gettime(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return errno::to_result(errno::EFAULT),
    };

    let linux_fd = trapframe.get_arg(0);
    let curr_value_ptr = trapframe.get_arg(1);

    trapframe.increment_pc_next(&task);

    if curr_value_ptr == 0 {
        return errno::to_result(errno::EFAULT);
    }

    let handle = match abi.get_handle(linux_fd) {
        Some(handle) => handle,
        None => return errno::to_result(errno::EBADF),
    };
    let kernel_obj = match task.handle_table.get(handle) {
        Some(kernel_obj) => kernel_obj,
        None => return errno::to_result(errno::EBADF),
    };
    let timer = match kernel_obj.as_timer() {
        Some(timer) => timer,
        None => return errno::to_result(errno::EINVAL),
    };

    let curr_value_paddr = match task.vm_manager.translate_to_kva(curr_value_ptr) {
        Some(addr) => addr as *mut ItimerSpec,
        None => return errno::to_result(errno::EFAULT),
    };
    let (remaining_ns, interval_ns) = timer.snapshot();
    let snapshot = ItimerSpec {
        it_interval: ns_to_timespec(interval_ns),
        it_value: ns_to_timespec(remaining_ns),
    };
    unsafe {
        *curr_value_paddr = snapshot;
    }

    0
}

/// Linux `timer_create` implementation.
///
/// Returns 0 on success or negative errno on failure.
pub fn sys_timer_create(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return errno::to_result(errno::EFAULT),
    };

    let clock_id = trapframe.get_arg(0) as i32;
    let sevp_ptr = trapframe.get_arg(1);
    let timerid_ptr = trapframe.get_arg(2);

    trapframe.increment_pc_next(&task);

    if !is_supported_clock(clock_id) {
        return errno::to_result(errno::EINVAL);
    }

    if timerid_ptr == 0 {
        return errno::to_result(errno::EFAULT);
    }

    let timerid_paddr = match task.vm_manager.translate_to_kva(timerid_ptr) {
        Some(ptr) => ptr as *mut u64,
        None => return errno::to_result(errno::EFAULT),
    };

    // Defaults if user does not supply struct sigevent
    let mut sigev_notify = SIGEV_SIGNAL;
    let mut sigev_signo = LinuxSignal::SIGALRM as i32;
    let mut sigev_value = 0u64;

    if sevp_ptr != 0 {
        let sevp_paddr = match task.vm_manager.translate_to_kva(sevp_ptr) {
            Some(addr) => addr,
            None => return errno::to_result(errno::EFAULT),
        };

        unsafe {
            let base = sevp_paddr as *const u8;
            sigev_value = *(base as *const u64);
            sigev_signo = *(base.add(8) as *const i32);
            sigev_notify = *(base.add(12) as *const i32);
        }

        match sigev_notify {
            SIGEV_SIGNAL => {
                if sigev_signo <= 0 || LinuxSignal::from_u32(sigev_signo as u32).is_none() {
                    return errno::to_result(errno::EINVAL);
                }
            }
            SIGEV_NONE => {
                // No additional validation needed
            }
            SIGEV_THREAD | SIGEV_THREAD_ID => {
                // Thread-based notifications require pthread helpers we do not have yet.
                return errno::to_result(errno::ENOSYS);
            }
            _ => {
                return errno::to_result(errno::EINVAL);
            }
        }
    }

    let timer_id = abi.allocate_posix_timer_id();
    let timer = PosixTimer::new(
        timer_id,
        clock_id,
        sigev_notify,
        sigev_signo,
        sigev_value,
        task.get_id(),
        abi.signal_state.clone(),
    );
    abi.store_posix_timer(timer);

    unsafe {
        *timerid_paddr = timer_id as u64;
    }

    trapframe.set_return_value(0);
    0
}

/// Linux `timer_settime` implementation.
pub fn sys_timer_settime(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return errno::to_result(errno::EFAULT),
    };

    let timer_id = trapframe.get_arg(0) as u64;
    let flags = trapframe.get_arg(1) as i32;
    let new_value_ptr = trapframe.get_arg(2);
    let old_value_ptr = trapframe.get_arg(3);

    trapframe.increment_pc_next(&task);

    if (flags & !TIMER_ABSTIME) != 0 {
        return errno::to_result(errno::EINVAL);
    }
    if (flags & TIMER_ABSTIME) != 0 {
        return errno::to_result(errno::ENOSYS);
    }
    if new_value_ptr == 0 {
        return errno::to_result(errno::EINVAL);
    }

    let timer = match abi.get_posix_timer(timer_id) {
        Some(timer) => timer,
        None => return errno::to_result(errno::EINVAL),
    };

    let new_value_paddr = match task.vm_manager.translate_to_kva(new_value_ptr) {
        Some(addr) => addr,
        None => return errno::to_result(errno::EFAULT),
    };

    let new_spec = unsafe { *(new_value_paddr as *const ItimerSpec) };

    let first_ns = match timespec_to_ns(&new_spec.it_value) {
        Ok(ns) => ns,
        Err(errno_val) => return errno::to_result(errno_val),
    };
    let interval_ns = match timespec_to_ns(&new_spec.it_interval) {
        Ok(ns) => ns,
        Err(errno_val) => return errno::to_result(errno_val),
    };

    if old_value_ptr != 0 {
        let old_value_paddr = match task.vm_manager.translate_to_kva(old_value_ptr) {
            Some(addr) => addr as *mut ItimerSpec,
            None => return errno::to_result(errno::EFAULT),
        };
        let (remaining_ns, previous_interval_ns) = timer.snapshot();
        let snapshot = ItimerSpec {
            it_interval: ns_to_timespec(previous_interval_ns),
            it_value: ns_to_timespec(remaining_ns),
        };
        unsafe {
            *old_value_paddr = snapshot;
        }
    }

    timer.schedule(first_ns, interval_ns);

    trapframe.set_return_value(0);
    0
}

/// Linux `timer_gettime` implementation.
pub fn sys_timer_gettime(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return errno::to_result(errno::EFAULT),
    };

    let timer_id = trapframe.get_arg(0) as u64;
    let setting_ptr = trapframe.get_arg(1);

    trapframe.increment_pc_next(&task);

    if setting_ptr == 0 {
        return errno::to_result(errno::EFAULT);
    }

    let timer = match abi.get_posix_timer(timer_id) {
        Some(timer) => timer,
        None => return errno::to_result(errno::EINVAL),
    };

    let setting_paddr = match task.vm_manager.translate_to_kva(setting_ptr) {
        Some(addr) => addr as *mut ItimerSpec,
        None => return errno::to_result(errno::EFAULT),
    };

    let (remaining_ns, interval_ns) = timer.snapshot();
    let current = ItimerSpec {
        it_interval: ns_to_timespec(interval_ns),
        it_value: ns_to_timespec(remaining_ns),
    };

    unsafe {
        *setting_paddr = current;
    }

    trapframe.set_return_value(0);
    0
}

/// Linux `timer_getoverrun` implementation (simple stub returning 0).
pub fn sys_timer_getoverrun(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return errno::to_result(errno::EFAULT),
    };

    let timer_id = trapframe.get_arg(0) as u64;

    trapframe.increment_pc_next(&task);

    let timer = match abi.get_posix_timer(timer_id) {
        Some(timer) => timer,
        None => return errno::to_result(errno::EINVAL),
    };

    let overrun = {
        let state = timer.state();
        state.overrun_count as usize
    };

    trapframe.set_return_value(overrun);
    overrun
}

/// Linux `timer_delete` implementation.
pub fn sys_timer_delete(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return errno::to_result(errno::EFAULT),
    };

    let timer_id = trapframe.get_arg(0) as u64;

    trapframe.increment_pc_next(&task);

    let timer = match abi.remove_posix_timer(timer_id) {
        Some(timer) => timer,
        None => return errno::to_result(errno::EINVAL),
    };

    timer.cancel();

    trapframe.set_return_value(0);
    0
}

/// sys_clock_gettime - Get time from specified clock
///
/// Arguments:
/// - a0 (x10): clock_id - which clock to read from
/// - a1 (x11): timespec - pointer to timespec structure to fill
///
/// Returns:
/// - 0 on success
/// - -EINVAL (-22) for invalid clock_id
/// - -EFAULT (-14) for invalid timespec pointer
pub fn sys_clock_gettime(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().expect("No current task found");
    let clock_id = trapframe.get_arg(0) as i32; // a0

    trapframe.increment_pc_next(&task);

    let timespec_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *mut TimeSpec,   // a1
        None => return (-14_isize) as usize, // -EFAULT
    };

    // Get the current time based on the clock type
    let timespec = match clock_id {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => {
            // Wall clock (Unix epoch). Falls back to monotonic if no RTC has
            // initialized the wall clock yet (e.g. RTC-less hardware), so that
            // clock_gettime never fails for REALTIME.
            let ns = crate::time::system_time_ns().unwrap_or_else(current_time_ns);
            ns_to_timespec(ns)
        }
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE => {
            // Monotonic time since boot
            let time_us = current_time();
            TimeSpec {
                tv_sec: (time_us / 1_000_000) as i64,
                tv_nsec: ((time_us % 1_000_000) * 1000) as i64,
            }
        }
        CLOCK_BOOTTIME => {
            // Boot time (same as monotonic for now)
            let time_us = current_time();
            TimeSpec {
                tv_sec: (time_us / 1_000_000) as i64,
                tv_nsec: ((time_us % 1_000_000) * 1000) as i64,
            }
        }
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // CPU time for process/thread (simplified implementation)
            // In a full implementation, this would track actual CPU time
            let time_us = current_time();
            TimeSpec {
                tv_sec: (time_us / 1_000_000) as i64,
                tv_nsec: ((time_us % 1_000_000) * 1000) as i64,
            }
        }
        _ => {
            return (-22_isize) as usize; // -EINVAL
        }
    };

    // Write the timespec to user space
    unsafe {
        *timespec_ptr = timespec;
    }

    0 // Success
}

/// sys_nanosleep - Sleep for the specified time (Linux ABI)
///
/// Arguments:
/// - a0 (x10): rqtp - pointer to requested sleep time (struct __kernel_timespec __user *)
/// - a1 (x11): rmtp - pointer to remaining time (struct __kernel_timespec __user *)
///
/// Returns:
/// - 0 on success
/// - -EFAULT (-14) for invalid pointer
/// - -EINTR (-4) if interrupted by signal (not implemented, always 0)
pub fn sys_nanosleep(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    // Get current task
    let task = match mytask() {
        Some(task) => task,
        None => return (-14_isize) as usize, // -EFAULT
    };
    trapframe.increment_pc_next(&task);

    // Get user pointer to requested timespec
    let rqtp_ptr = trapframe.get_arg(0);
    let _rmtp_ptr = trapframe.get_arg(1);
    let rqtp = match task.vm_manager.translate_to_kva(rqtp_ptr) {
        Some(ptr) => unsafe { &*(ptr as *const TimeSpec) },
        None => return (-14_isize) as usize, // -EFAULT
    };
    // Convert timespec to nanoseconds
    let ns = rqtp
        .tv_sec
        .saturating_mul(1_000_000_000)
        .saturating_add(rqtp.tv_nsec);
    if ns <= 0 {
        return 0;
    }
    // Convert nanoseconds to kernel ticks
    let ticks = ns_to_ticks(ns as u64);
    trapframe.set_return_value(0); // Set return value to 0 (success)
    // Sleep the current task for the specified ticks
    task.sleep(trapframe, ticks);
    // If sleep is successful, this will not be reached. If interrupted, return -EINTR (not implemented)
    0
}

/// Linux sys_clock_getres implementation (stub)
///
/// Get clock resolution. This is a stub implementation that
/// returns a reasonable resolution for the specified clock.
///
/// Arguments:
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: clk_id (clock ID)
///   - arg1: res (pointer to timespec structure for resolution)
///
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_clock_getres(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _clk_id = trapframe.get_arg(0) as i32;
    let res_ptr = trapframe.get_arg(1);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // If res pointer is provided, write resolution
    if res_ptr != 0 {
        if let Some(res_paddr) = task.vm_manager.translate_to_kva(res_ptr) {
            unsafe {
                // Write timespec structure with nanosecond resolution
                // struct timespec { long tv_sec; long tv_nsec; }
                let timespec = res_paddr as *mut [u64; 2];
                *timespec = [
                    0,         // tv_sec = 0
                    1_000_000, // tv_nsec = 1 millisecond (reasonable resolution)
                ];
            }
        }
    }

    0 // Always succeed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_timespec_size() {
        // Ensure TimeSpec matches Linux ABI
        assert_eq!(core::mem::size_of::<TimeSpec>(), 16);
        assert_eq!(core::mem::align_of::<TimeSpec>(), 8);
    }

    #[test_case]
    fn test_clock_constants() {
        // Verify clock constants match Linux values
        assert_eq!(CLOCK_REALTIME, 0);
        assert_eq!(CLOCK_MONOTONIC, 1);
        assert_eq!(CLOCK_PROCESS_CPUTIME_ID, 2);
        assert_eq!(CLOCK_THREAD_CPUTIME_ID, 3);
    }
}
