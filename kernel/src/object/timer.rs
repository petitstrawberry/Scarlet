//! Kernel timer objects.

use crate::sync::IrqSpinLock;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::object::capability::{StreamError, StreamOps};
use crate::sched::scheduler::current_task_id;
use crate::sync::waker::Waker;
use crate::timer::{
    TimerHandle, TimerHandler as SoftwareTimerHandler, add_timer, cancel_timer,
    coalesce_periodic_deadline, get_time_ns,
};

/// Kernel timer object usable through ABI handle layers.
pub trait TimerObject: StreamOps + Selectable {
    /// Return the clock id associated with this timer.
    ///
    /// # Returns
    ///
    /// The ABI clock id supplied when the timer was created.
    fn clock_id(&self) -> i32;

    /// Arm or disarm the timer.
    fn set_time(&self, first_ns: u64, interval_ns: u64, absolute: bool);

    /// Arm against an absolute native wall-clock deadline, or watch clock
    /// changes while disarmed. Discontinuities either rebase or cancel it.
    fn set_wall_time(&self, first_ns: u64, interval_ns: u64, cancel_on_change: bool) -> Result<(), &'static str>;

    fn is_wakeup(&self) -> bool { false }

    /// Return remaining time and interval, both in nanoseconds.
    fn snapshot(&self) -> (u64, u64);

    /// Cancel any outstanding timer.
    fn cancel(&self);
}

struct TimerState {
    interval_ns: u64,
    timer_entry_id: Option<TimerHandle>,
    next_deadline_ns: Option<u64>,
    expirations: u64,
    active: bool,
    generation: u64,
    source_deadline_ns: Option<u64>,
    wall_clock: bool,
    cancel_on_change: bool,
    cancelled: bool,
    clock_revision: u64,
    observing_clock: bool,
    wake: Option<crate::system_power::wake::Reservation>,
}

struct TimerShared {
    state: IrqSpinLock<TimerState>,
    read_waker: Waker,
}

impl TimerShared {
    fn new(wake: Option<crate::system_power::wake::Reservation>) -> Arc<Self> {
        Arc::new(Self {
            state: IrqSpinLock::new(TimerState {
                interval_ns: 0,
                timer_entry_id: None,
                next_deadline_ns: None,
                expirations: 0,
                active: false,
                generation: 0,
                source_deadline_ns: None,
                wall_clock: false,
                cancel_on_change: false,
                cancelled: false,
                clock_revision: 0,
                observing_clock: false,
                wake,
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
        Self::with_reservation(clock_id, nonblocking, None)
    }

    pub fn new_wakeup(clock_id: i32, nonblocking: bool) -> Result<Self, &'static str> {
        Ok(Self::with_reservation(clock_id, nonblocking, Some(crate::system_power::wake::Reservation::new()?)))
    }

    fn with_reservation(clock_id: i32, nonblocking: bool, wake: Option<crate::system_power::wake::Reservation>) -> Self {
        let shared = TimerShared::new(wake);
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

    fn first_deadline_ns(first_ns: u64, absolute: bool) -> Option<u64> {
        if first_ns == 0 {
            return None;
        }

        Some(if absolute {
            first_ns
        } else {
            get_time_ns().saturating_add(first_ns)
        })
    }

    fn configure(&self, first_ns: u64, interval_ns: u64, absolute: bool, wall_clock: bool, cancel_on_change: bool) -> Result<(), &'static str> {
        let mut state = self.shared.state.lock();
        if wall_clock && !state.observing_clock {
            let observer: Arc<dyn crate::time::ClockChangeListener> = self.handler.clone();
            crate::time::observe_clock_changes(&observer)?;
            state.observing_clock = true;
        }
        if let Some(entry_id) = state.timer_entry_id.take() { cancel_timer(entry_id); }
        state.generation = state.generation.wrapping_add(1);
        state.interval_ns = interval_ns;
        state.next_deadline_ns = None;
        state.source_deadline_ns = Self::first_deadline_ns(first_ns, absolute);
        state.expirations = 0;
        state.active = false;
        state.wall_clock = wall_clock;
        state.cancel_on_change = cancel_on_change;
        state.cancelled = false;
        state.clock_revision = crate::time::clock_revision();
        if let Some(deadline) = state.source_deadline_ns {
            self.handler.schedule_locked(&mut state, deadline);
        }
        state.publish_wake();
        Ok(())
    }
}

impl TimerState {
    fn publish_wake(&self) {
        if let Some(wake) = &self.wake {
            wake.update(self.next_deadline_ns, self.cancelled || self.expirations != 0);
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl TimerObject for Timer {
    fn clock_id(&self) -> i32 {
        self.clock_id
    }

    fn set_time(&self, first_ns: u64, interval_ns: u64, absolute: bool) {
        let _ = self.configure(first_ns, interval_ns, absolute, false, false);
    }

    fn set_wall_time(&self, first_ns: u64, interval_ns: u64, cancel_on_change: bool) -> Result<(), &'static str> {
        self.configure(first_ns, interval_ns, true, true, cancel_on_change)
    }

    fn is_wakeup(&self) -> bool {
        self.shared.state.lock().wake.is_some()
    }

    fn snapshot(&self) -> (u64, u64) {
        let state = self.shared.state.lock();
        let remaining = match state.next_deadline_ns {
            Some(deadline) => deadline.saturating_sub(get_time_ns()),
            None => 0,
        };
        (remaining, state.interval_ns)
    }

    fn cancel(&self) {
        let mut state = self.shared.state.lock();
        if let Some(entry_id) = state.timer_entry_id.take() {
            cancel_timer(entry_id);
        }
        state.generation = state.generation.wrapping_add(1);
        state.next_deadline_ns = None;
        state.source_deadline_ns = None;
        state.active = false;
        state.cancel_on_change = false;
        state.cancelled = false;
        state.expirations = 0;
        state.publish_wake();
    }
}

impl StreamOps for Timer {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if buffer.len() < core::mem::size_of::<u64>() {
            return Err(StreamError::InvalidArgument);
        }

        let value = {
            let mut state = self.shared.state.lock();
            if state.cancelled {
                state.cancelled = false;
                state.publish_wake();
                return Err(StreamError::Cancelled);
            }
            if state.expirations == 0 {
                return Err(StreamError::WouldBlock);
            }
            let value = state.expirations;
            state.expirations = 0;
            state.publish_wake();
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
            set.read = state.expirations > 0 || state.cancelled;
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
        timeout_ns: Option<u64>,
        min_wait_ns: u64,
    ) -> SelectWaitOutcome {
        let current = self.current_ready(interest);
        if (interest.read && current.read) || (interest.write && current.write) {
            return SelectWaitOutcome::Ready;
        }

        let task_id = {
            let cpu_id = crate::arch::get_cpu().get_cpuid();
            current_task_id(cpu_id).unwrap_or(0)
        };

        let woke = if min_wait_ns > 0 {
            self.shared.read_waker.wait_with_min_timeout(
                task_id,
                trapframe,
                timeout_ns,
                min_wait_ns,
            )
        } else {
            self.shared
                .read_waker
                .wait_with_timeout(task_id, trapframe, timeout_ns)
        };

        let after = self.current_ready(interest);
        if timeout_ns.is_some() && !woke && !after.read && !after.write {
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

fn wall_deadline_to_monotonic(deadline: u64, wall_now: u64, monotonic_now: u64) -> u64 {
    monotonic_now.saturating_add(deadline.saturating_sub(wall_now)).max(1)
}

impl TimerCallback {
    fn schedule_locked(self: &Arc<Self>, state: &mut TimerState, source_deadline_ns: u64) {
        let mono = get_time_ns();
        let target = if state.wall_clock {
            let wall = crate::time::system_time_ns().unwrap_or(mono);
            wall_deadline_to_monotonic(source_deadline_ns, wall, mono)
        } else { source_deadline_ns };
        let handler: Arc<dyn SoftwareTimerHandler> = self.clone();
        state.timer_entry_id = Some(add_timer(target, crate::timer::TimerPrecision::Normal, &handler, state.generation as usize));
        state.source_deadline_ns = Some(source_deadline_ns);
        state.next_deadline_ns = Some(target);
        state.active = true;
    }
}

impl crate::time::ClockChangeListener for TimerCallback {
    fn clock_changed(self: Arc<Self>, revision: u64) {
        let Some(shared) = self.shared.upgrade() else { return; };
        let mut state = shared.state.lock();
        // Notifications from concurrent clock updates can arrive out of order.
        if revision <= state.clock_revision { return; }
        state.clock_revision = revision;
        if !state.wall_clock { return; }
        if let Some(entry_id) = state.timer_entry_id.take() { cancel_timer(entry_id); }
        state.generation = state.generation.wrapping_add(1);
        if state.cancel_on_change {
            state.cancelled = true;
            state.active = false;
            state.next_deadline_ns = None;
            state.source_deadline_ns = None;
            state.expirations = 0;
        } else if let Some(deadline) = state.source_deadline_ns {
            self.schedule_locked(&mut state, deadline);
        }
        state.publish_wake();
        drop(state);
        shared.read_waker.wake_all();
    }
}

impl SoftwareTimerHandler for TimerCallback {
    fn on_timer_expired(self: Arc<Self>, context: usize) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };

        let generation = context as u64;
        {
            let mut state = shared.state.lock();
            if !state.active || state.generation != generation {
                return;
            }

            state.timer_entry_id = None;
            let now_ns = if state.wall_clock {
                crate::time::system_time_ns().unwrap_or_else(get_time_ns)
            } else { get_time_ns() };
            let expired_deadline_ns = state.source_deadline_ns.take().unwrap_or(now_ns);
            state.next_deadline_ns = None;
            if state.interval_ns > 0 {
                let (target_deadline_ns, overruns) =
                    coalesce_periodic_deadline(expired_deadline_ns, state.interval_ns, now_ns);
                state.expirations = state.expirations.saturating_add(overruns.saturating_add(1));
                self.schedule_locked(&mut state, target_deadline_ns);
            } else {
                state.expirations = state.expirations.saturating_add(1);
                state.active = false;
            }
            state.publish_wake();
        }

        shared.read_waker.wake_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::ClockChangeListener;

    #[test_case]
    fn wall_deadline_conversion_preserves_remaining_time_and_saturates() {
        assert_eq!(wall_deadline_to_monotonic(1050,1000,200),250);
        assert_eq!(wall_deadline_to_monotonic(999,1000,200),200);
        assert_eq!(wall_deadline_to_monotonic(999,1000,0),1);
        assert_eq!(wall_deadline_to_monotonic(u64::MAX,0,1),u64::MAX);
    }

    #[test_case]
    fn wake_event_remains_pending_until_read_and_last_handle_releases_reservation() {
        let before = crate::system_power::wake::snapshot();
        let timer = Arc::new(Timer::new_wakeup(9, true).unwrap());
        timer.set_time(1_000_000_000_000, 0, false);
        assert!(crate::system_power::wake::snapshot().deadline_ns.is_some());
        let generation = {
            let state = timer.shared.state.lock();
            // Simulate the queue removing the due entry before dispatch.
            cancel_timer(state.timer_entry_id.unwrap());
            state.generation
        };
        timer.handler.clone().on_timer_expired(generation as usize);
        assert_eq!(crate::system_power::wake::snapshot().pending_events, before.pending_events + 1);
        let mut bytes = [0;8]; assert_eq!(timer.read(&mut bytes).unwrap(), 8);
        assert_eq!(u64::from_ne_bytes(bytes), 1);
        assert_eq!(crate::system_power::wake::snapshot().pending_events, before.pending_events);
        timer.set_time(1_000_000_000_000, 0, false);
        let duplicate = timer.clone(); drop(timer);
        assert!(crate::system_power::wake::snapshot().deadline_ns.is_some());
        drop(duplicate);
        assert_eq!(crate::system_power::wake::snapshot().deadline_ns, before.deadline_ns);
    }

    #[test_case]
    fn disarmed_clock_change_monitor_is_readable_and_reports_cancellation_once() {
        let timer = Timer::new(0, true);
        timer.set_wall_time(0, 0, true).unwrap();
        timer.handler.clone().clock_changed(crate::time::clock_revision() + 1);
        assert!(timer.current_ready(ReadyInterest { read:true, write:false, except:false }).read);
        assert!(matches!(timer.read(&mut [0;8]), Err(StreamError::Cancelled)));
        assert!(matches!(timer.read(&mut [0;8]), Err(StreamError::WouldBlock)));
    }

    #[test_case]
    fn wall_clock_rebase_and_cancel_reject_stale_timer_callbacks() {
        let timer = Timer::new_wakeup(8, true).unwrap();
        let wall = crate::time::system_time_ns().unwrap_or_else(get_time_ns);
        timer.set_wall_time(wall.saturating_add(1_000_000_000_000), 0, false).unwrap();
        let old = timer.shared.state.lock().generation;
        timer.handler.clone().clock_changed(crate::time::clock_revision() + 1);
        let new = timer.shared.state.lock().generation; assert_ne!(old,new);
        timer.handler.clone().on_timer_expired(old as usize);
        assert!(matches!(timer.read(&mut [0;8]), Err(StreamError::WouldBlock)));
        timer.cancel(); timer.handler.clone().on_timer_expired(new as usize);
        assert!(matches!(timer.read(&mut [0;8]), Err(StreamError::WouldBlock)));
    }
}
