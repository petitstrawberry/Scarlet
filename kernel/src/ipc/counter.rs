//! Counter IPC Object
//!
//! This module provides a counter-based IPC mechanism similar to Linux's eventfd.
//! Multiple file descriptors can share the same counter state (for dup/fork).
//!
//! Behavior:
//! - read(8 bytes): Returns counter value and resets to 0
//!   (or decrements by 1 in semaphore mode)
//! - write(8 bytes): Adds value to counter

use alloc::{string::String, string::ToString, sync::Arc, vec::Vec};
use spin::Mutex;

use crate::object::KernelObject;
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::object::capability::{CloneOps, StreamError, StreamOps};
use crate::sched::scheduler::current_task_id;
use crate::sync::waker::Waker;

/// Observer notified when a counter is written.
pub trait CounterWriteListener: Send + Sync {
    fn on_counter_write(&self, value: u64);
}

/// Internal state of a counter
struct CounterState {
    /// 64-bit counter value
    counter: u64,
    /// Semaphore mode flag (decrement by 1 on read instead of reset)
    semaphore: bool,
}

/// Shared counter data including state and wakers
struct SharedCounterData {
    /// Protected state
    state: Mutex<CounterState>,
    /// Waker for tasks waiting to read
    read_waker: Waker,
    /// Waker for tasks waiting to write
    write_waker: Waker,
    /// Listeners notified after successful writes
    write_listeners: Mutex<Vec<Arc<dyn CounterWriteListener>>>,
}

impl SharedCounterData {
    fn new(initval: u32, semaphore: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(CounterState {
                counter: initval as u64,
                semaphore,
            }),
            read_waker: Waker::new_interruptible("counter_read"),
            write_waker: Waker::new_interruptible("counter_write"),
            write_listeners: Mutex::new(Vec::new()),
        })
    }
}

/// Counter object for eventfd-like IPC
pub struct Counter {
    /// Shared data
    data: Arc<SharedCounterData>,
    /// Unique identifier for debugging
    id: String,
    /// Non-blocking flag
    nonblocking: core::sync::atomic::AtomicBool,
}

impl Counter {
    /// Create a new counter object
    pub fn new(initval: u32, semaphore: bool) -> Self {
        Self {
            data: SharedCounterData::new(initval, semaphore),
            id: "counter".to_string(),
            nonblocking: core::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Create a new counter pair as KernelObjects (returns same counter twice for dup semantics)
    pub fn create_pair(initval: u32, semaphore: bool) -> (KernelObject, KernelObject) {
        let data = SharedCounterData::new(initval, semaphore);

        let counter1 = Self {
            data: data.clone(),
            id: "counter_1".to_string(),
            nonblocking: core::sync::atomic::AtomicBool::new(false),
        };

        let counter2 = Self {
            data: data.clone(),
            id: "counter_2".to_string(),
            nonblocking: core::sync::atomic::AtomicBool::new(false),
        };

        // Wrap in KernelObjects as Counter
        let obj1 = KernelObject::from_counter(Arc::new(counter1));
        let obj2 = KernelObject::from_counter(Arc::new(counter2));

        (obj1, obj2)
    }

    /// Create a single counter as KernelObject
    pub fn create_kernel_object(initval: u32, flags: u32) -> KernelObject {
        const EFD_SEMAPHORE: u32 = 0x00000001;
        const EFD_NONBLOCK: u32 = 0o00004000;

        let semaphore = (flags & EFD_SEMAPHORE) != 0;
        let nonblocking = (flags & EFD_NONBLOCK) != 0;

        let mut counter = Self::new(initval, semaphore);
        if nonblocking {
            counter
                .nonblocking
                .store(true, core::sync::atomic::Ordering::Relaxed);
        }

        KernelObject::from_counter(Arc::new(counter))
    }

    /// Read the counter value (8 bytes)
    fn do_read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if buffer.len() < 8 {
            return Err(StreamError::InvalidArgument);
        }

        loop {
            let mut state = self.data.state.lock();

            if state.counter == 0 {
                // Counter is 0 - would block
                if self.nonblocking.load(core::sync::atomic::Ordering::Relaxed) {
                    return Err(StreamError::WouldBlock);
                }

                // For now, just return WouldBlock (TODO: implement proper blocking)
                return Err(StreamError::WouldBlock);
            }

            // Read the value
            let value = if state.semaphore {
                // Semaphore mode: read 1
                state.counter -= 1;
                1u64
            } else {
                // Normal mode: read and reset to 0
                let value = state.counter;
                state.counter = 0;
                value
            };

            // Release lock before waking writers
            drop(state);

            // Write to buffer (native endianness)
            buffer[0..8].copy_from_slice(&value.to_ne_bytes());

            // Wake up any waiting writers
            self.data.write_waker.wake_all();

            return Ok(8);
        }
    }

    /// Write to the counter (8 bytes)
    fn do_write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        if buffer.len() < 8 {
            return Err(StreamError::InvalidArgument);
        }

        // Parse the value to add
        let mut value_bytes = [0u8; 8];
        value_bytes.copy_from_slice(&buffer[0..8]);
        let add_value = u64::from_ne_bytes(value_bytes);

        // Prevent overflow (max value is UINT64_MAX - 1)
        if add_value == u64::MAX {
            return Err(StreamError::InvalidArgument);
        }

        loop {
            let mut state = self.data.state.lock();

            // Check if adding would overflow
            if state.counter > u64::MAX - add_value - 1 {
                // Would overflow - block
                if self.nonblocking.load(core::sync::atomic::Ordering::Relaxed) {
                    return Err(StreamError::WouldBlock);
                }

                // For now, just return WouldBlock (TODO: implement proper blocking)
                return Err(StreamError::WouldBlock);
            }

            // Add to counter
            state.counter = state.counter.wrapping_add(add_value);

            // Release lock before waking readers
            drop(state);

            // Wake up any waiting readers
            self.data.read_waker.wake_all();

            if add_value != 0 {
                let listeners = self.data.write_listeners.lock().clone();
                for listener in listeners {
                    listener.on_counter_write(add_value);
                }
            }

            return Ok(8);
        }
    }

    /// Add an observer notified after successful writes.
    pub fn add_write_listener(&self, listener: Arc<dyn CounterWriteListener>) {
        self.data.write_listeners.lock().push(listener);
    }
}

impl StreamOps for Counter {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        self.do_read(buffer)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        self.do_write(buffer)
    }
}

impl CloneOps for Counter {
    fn custom_clone(&self) -> KernelObject {
        // Counter can be cloned (creates new fd pointing to same counter)
        KernelObject::from_counter(Arc::new(self.clone()))
    }
}

impl Clone for Counter {
    fn clone(&self) -> Self {
        let mut new_id = String::from(self.id.as_str());
        new_id.push_str("_clone");
        Self {
            data: self.data.clone(),
            id: new_id,
            nonblocking: core::sync::atomic::AtomicBool::new(
                self.nonblocking.load(core::sync::atomic::Ordering::Relaxed),
            ),
        }
    }
}

impl Selectable for Counter {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        let state = self.data.state.lock();

        if interest.read {
            // Readable if counter > 0
            set.read = state.counter > 0;
        }
        if interest.write {
            // Writable if counter < UINT64_MAX - 1
            set.write = state.counter < u64::MAX - 1;
        }
        if interest.except {
            set.except = false;
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
            use crate::arch::get_cpu;
            let cpu_id = get_cpu().get_cpuid();
            current_task_id(cpu_id).unwrap_or(0)
        };

        let woke = if interest.read {
            if min_wait_ticks > 0 {
                self.data.read_waker.wait_with_min_timeout(
                    task_id,
                    trapframe,
                    timeout_ticks,
                    min_wait_ticks,
                )
            } else {
                self.data
                    .read_waker
                    .wait_with_timeout(task_id, trapframe, timeout_ticks)
            }
        } else if interest.write {
            if min_wait_ticks > 0 {
                self.data.write_waker.wait_with_min_timeout(
                    task_id,
                    trapframe,
                    timeout_ticks,
                    min_wait_ticks,
                )
            } else {
                self.data
                    .write_waker
                    .wait_with_timeout(task_id, trapframe, timeout_ticks)
            }
        } else {
            false
        };

        let after = self.current_ready(interest);
        if timeout_ticks.is_some() && !woke && !after.read && !after.write {
            SelectWaitOutcome::TimedOut
        } else {
            SelectWaitOutcome::Ready
        }
    }

    fn set_nonblocking(&self, enabled: bool) {
        self.nonblocking
            .store(enabled, core::sync::atomic::Ordering::Relaxed);
    }

    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(core::sync::atomic::Ordering::Relaxed)
    }
}

/// Counter-specific operations trait
pub trait CounterObject: StreamOps + Selectable + CloneOps {
    /// Check if this is a semaphore mode counter
    fn is_semaphore(&self) -> bool;

    /// Add an observer notified after successful writes.
    fn add_write_listener(&self, listener: Arc<dyn CounterWriteListener>);
}

impl CounterObject for Counter {
    fn is_semaphore(&self) -> bool {
        self.data.state.lock().semaphore
    }

    fn add_write_listener(&self, listener: Arc<dyn CounterWriteListener>) {
        self.add_write_listener(listener);
    }
}
