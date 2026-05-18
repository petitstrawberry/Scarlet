//! Kernel timer objects.

use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::object::capability::{StreamError, StreamOps};
use crate::sched::scheduler::current_task_id;
use crate::sync::waker::Waker;
use crate::timer::{
    TimerHandler as SoftwareTimerHandler, add_timer, cancel_timer, get_tick, ns_to_ticks,
    ticks_to_ns,
};

/// Kernel timer object usable through ABI handle layers.
pub trait TimerObject: StreamOps + Selectable {
    /// Arm or disarm the timer.
    fn set_time(&self, first_ns: u64, interval_ns: u64, absolute: bool);

    /// Return remaining time and interval, both in nanoseconds.
    fn snapshot(&self) -> (u64, u64);

    /// Cancel any outstanding timer.
    fn cancel(&self);
}

struct TimerState {
    interval_ns: u64,
    timer_entry_id: Option<u64>,
    next_deadline_tick: Option<u64>,
    expirations: u64,
    active: bool,
}

struct TimerShared {
    state: Mutex<TimerState>,
    read_waker: Waker,
}

impl TimerShared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TimerState {
                interval_ns: 0,
                timer_entry_id: None,
                next_deadline_tick: None,
                expirations: 0,
                active: false,
            }),
            read_waker: Waker::new_interruptible("timer_read"),
        })
    }
}

/// Kernel-backed timer object.
pub struct Timer {
    clock_id: i32,
    shared: Arc<TimerShared>,
    handler: Arc<TimerCallback>,
    nonblocking: AtomicBool,
}

impl Timer {
    /// Create a new disarmed timer object.
    pub fn new(clock_id: i32, nonblocking: bool) -> Self {
        let shared = TimerShared::new();
        let handler = Arc::new(TimerCallback {
            shared: Arc::downgrade(&shared),
        });
        Self {
            clock_id,
            shared,
            handler,
            nonblocking: AtomicBool::new(nonblocking),
        }
    }

    /// Clock id used when this timer was created.
    pub fn clock_id(&self) -> i32 {
        self.clock_id
    }

    fn delay_ns_to_ticks(ns: u64) -> u64 {
        if ns == 0 {
            0
        } else {
            core::cmp::max(ns_to_ticks(ns), 1)
        }
    }

    fn first_deadline_tick(first_ns: u64, absolute: bool) -> Option<u64> {
        if first_ns == 0 {
            return None;
        }

        let now = get_tick();
        if absolute {
            let now_ns = ticks_to_ns(now);
            let delay_ns = first_ns.saturating_sub(now_ns);
            Some(now.saturating_add(Self::delay_ns_to_ticks(delay_ns).max(1)))
        } else {
            Some(now.saturating_add(Self::delay_ns_to_ticks(first_ns)))
        }
    }

    fn arm_at_locked(&self, state: &mut TimerState, target_tick: u64) {
        let handler_dyn: Arc<dyn SoftwareTimerHandler> = self.handler.clone();
        let entry_id = add_timer(target_tick, &handler_dyn, 0);
        state.timer_entry_id = Some(entry_id);
        state.next_deadline_tick = Some(target_tick);
        state.active = true;
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl TimerObject for Timer {
    fn set_time(&self, first_ns: u64, interval_ns: u64, absolute: bool) {
        let mut state = self.shared.state.lock();
        if let Some(entry_id) = state.timer_entry_id.take() {
            cancel_timer(entry_id);
        }

        state.interval_ns = interval_ns;
        state.next_deadline_tick = None;
        state.expirations = 0;
        state.active = false;

        if let Some(target_tick) = Self::first_deadline_tick(first_ns, absolute) {
            self.arm_at_locked(&mut state, target_tick);
        }
    }

    fn snapshot(&self) -> (u64, u64) {
        let state = self.shared.state.lock();
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
        (remaining, state.interval_ns)
    }

    fn cancel(&self) {
        let mut state = self.shared.state.lock();
        if let Some(entry_id) = state.timer_entry_id.take() {
            cancel_timer(entry_id);
        }
        state.next_deadline_tick = None;
        state.active = false;
    }
}

impl StreamOps for Timer {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if buffer.len() < core::mem::size_of::<u64>() {
            return Err(StreamError::InvalidArgument);
        }

        let value = {
            let mut state = self.shared.state.lock();
            if state.expirations == 0 {
                return Err(StreamError::WouldBlock);
            }
            let value = state.expirations;
            state.expirations = 0;
            value
        };

        buffer[..8].copy_from_slice(&value.to_ne_bytes());
        Ok(8)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::InvalidArgument)
    }
}

impl Selectable for Timer {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        let state = self.shared.state.lock();
        if interest.read {
            set.read = state.expirations > 0;
        }
        if interest.write {
            set.write = false;
        }
        set
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ticks: Option<u64>,
        min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        let current = self.current_ready(interest);
        if (interest.read && current.read) || (interest.write && current.write) {
            return SelectWaitOutcome::Ready;
        }

        let task_id = {
            let cpu_id = crate::arch::get_cpu().get_cpuid();
            current_task_id(cpu_id).unwrap_or(0)
        };

        let woke = if min_wait_ticks > 0 {
            self.shared.read_waker.wait_with_min_timeout(
                task_id,
                trapframe,
                timeout_ticks,
                min_wait_ticks,
            )
        } else {
            self.shared
                .read_waker
                .wait_with_timeout(task_id, trapframe, timeout_ticks)
        };

        let after = self.current_ready(interest);
        if timeout_ticks.is_some() && !woke && !after.read && !after.write {
            SelectWaitOutcome::TimedOut
        } else {
            SelectWaitOutcome::Ready
        }
    }

    fn set_nonblocking(&self, enabled: bool) {
        self.nonblocking.store(enabled, Ordering::Relaxed);
    }

    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Relaxed)
    }
}

struct TimerCallback {
    shared: Weak<TimerShared>,
}

impl SoftwareTimerHandler for TimerCallback {
    fn on_timer_expired(self: Arc<Self>, _context: usize) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };

        {
            let mut state = shared.state.lock();
            if !state.active {
                return;
            }

            state.timer_entry_id = None;
            state.next_deadline_tick = None;
            state.expirations = state.expirations.saturating_add(1);

            if state.interval_ns > 0 {
                let delay_ticks = Timer::delay_ns_to_ticks(state.interval_ns);
                let target_tick = get_tick().saturating_add(delay_ticks);
                let handler_dyn: Arc<dyn SoftwareTimerHandler> = self.clone();
                let entry_id = add_timer(target_tick, &handler_dyn, 0);
                state.timer_entry_id = Some(entry_id);
                state.next_deadline_tick = Some(target_tick);
                state.active = true;
            } else {
                state.active = false;
            }
        }

        shared.read_waker.wake_all();
    }
}
