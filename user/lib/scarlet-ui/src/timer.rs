//! Timer support for scheduled UI updates
//!
//! Provides mechanisms for scheduling periodic or one-shot updates
//! to the UI from background tasks.
//!
//! # Example
//!
//! ```no_run
//! use scarlet_ui::{Timer, State};
//! use core::time::Duration;
//!
//! let counter = State::new(0);
//! let counter_clone = counter.clone();
//!
//! Timer::periodic(Duration::from_secs(1), move || {
//!     counter_clone.modify(|c| *c += 1);
//! });
//! ```

use scarlet_std::boxed::Box;
use scarlet_std::sync::{Arc, Mutex};
use scarlet_std::vec::Vec;
use core::time::Duration;

/// Callback type for timer events
type TimerCallback = Box<dyn FnMut() + Send + 'static>;

/// Global timer registry
static TIMERS: Mutex<Vec<TimerEntry>> = Mutex::new(Vec::new());

/// Internal timer entry
struct TimerEntry {
    id: u64,
    interval: Duration,
    callback: TimerCallback,
    last_tick: u64,
    one_shot: bool,
    cancelled: bool,
}

static NEXT_TIMER_ID: Mutex<u64> = Mutex::new(1);

fn next_timer_id() -> u64 {
    let mut id = NEXT_TIMER_ID.lock();
    let current = *id;
    *id += 1;
    current
}

/// Timer handle for cancellation
pub struct Timer {
    id: u64,
}

impl Timer {
    /// Create a periodic timer that fires at regular intervals
    ///
    /// # Example
    ///
    /// ```no_run
    /// use scarlet_ui::Timer;
    /// use core::time::Duration;
    ///
    /// let timer = Timer::periodic(Duration::from_millis(100), || {
    ///     println!("Tick!");
    /// });
    /// ```
    pub fn periodic<F>(interval: Duration, callback: F) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        let id = next_timer_id();
        let entry = TimerEntry {
            id,
            interval,
            callback: Box::new(callback),
            last_tick: 0, // Will be set on first process
            one_shot: false,
            cancelled: false,
        };
        
        TIMERS.lock().push(entry);
        
        Self { id }
    }

    /// Create a one-shot timer that fires once after a delay
    ///
    /// # Example
    ///
    /// ```no_run
    /// use scarlet_ui::Timer;
    /// use core::time::Duration;
    ///
    /// Timer::once(Duration::from_secs(2), || {
    ///     println!("Delayed action");
    /// });
    /// ```
    pub fn once<F>(delay: Duration, callback: F) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        let id = next_timer_id();
        let entry = TimerEntry {
            id,
            interval: delay,
            callback: Box::new(callback),
            last_tick: 0,
            one_shot: true,
            cancelled: false,
        };
        
        TIMERS.lock().push(entry);
        
        Self { id }
    }

    /// Cancel this timer
    pub fn cancel(&self) {
        let mut timers = TIMERS.lock();
        if let Some(timer) = timers.iter_mut().find(|t| t.id == self.id) {
            timer.cancelled = true;
        }
    }
}

/// Process all active timers (called by the event loop)
///
/// This should be called periodically by the application event loop.
pub fn process_timers(current_time_ms: u64) {
    let mut timers = TIMERS.lock();
    
    // Remove cancelled timers
    timers.retain(|t| !t.cancelled);
    
    // Process active timers
    let mut to_remove = Vec::new();
    
    for (idx, timer) in timers.iter_mut().enumerate() {
        if timer.last_tick == 0 {
            timer.last_tick = current_time_ms;
            continue;
        }
        
        let elapsed = current_time_ms.saturating_sub(timer.last_tick);
        let interval_ms = timer.interval.as_millis() as u64;
        
        if elapsed >= interval_ms {
            // Fire the callback
            (timer.callback)();
            
            if timer.one_shot {
                to_remove.push(idx);
            } else {
                timer.last_tick = current_time_ms;
            }
        }
    }
    
    // Remove one-shot timers that have fired
    for idx in to_remove.into_iter().rev() {
        timers.remove(idx);
    }
}

/// Schedule a callback to run on the main thread
///
/// This is useful for updating UI from background threads.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::schedule_on_main_thread;
///
/// // From a background thread:
/// schedule_on_main_thread(|| {
///     // This will run on the main UI thread
///     println!("UI update");
/// });
/// ```
pub fn schedule_on_main_thread<F>(callback: F)
where
    F: FnOnce() + Send + 'static,
{
    static MAIN_THREAD_QUEUE: Mutex<Vec<Box<dyn FnOnce() + Send>>> = 
        Mutex::new(Vec::new());
    
    MAIN_THREAD_QUEUE.lock().push(Box::new(callback));
}

/// Process all pending main thread callbacks
///
/// This should be called by the event loop on the main thread.
pub fn process_main_thread_queue() {
    static MAIN_THREAD_QUEUE: Mutex<Vec<Box<dyn FnOnce() + Send>>> = 
        Mutex::new(Vec::new());
    
    let mut queue = MAIN_THREAD_QUEUE.lock();
    let callbacks: Vec<_> = queue.drain(..).collect();
    drop(queue); // Release lock before executing callbacks
    
    for callback in callbacks {
        callback();
    }
}
