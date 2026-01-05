//! Event device implementation for input events
//!
//! This module provides an event device that implements the CharDevice trait
//! for handling input events from keyboards, mice, touchscreens, etc.

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

/// Static counters for device naming
static KEYBOARD_COUNTER: AtomicUsize = AtomicUsize::new(0);
static MOUSE_COUNTER: AtomicUsize = AtomicUsize::new(0);
static TABLET_COUNTER: AtomicUsize = AtomicUsize::new(0);
static INPUT_COUNTER: AtomicUsize = AtomicUsize::new(0);

use crate::arch::Trapframe;
use crate::device::char::CharDevice;
use crate::device::{Device, DeviceType};
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::sync::Waker;

use super::InputEvent;

/// Maximum number of events to buffer
const EVENT_QUEUE_CAPACITY: usize = 64;

/// Event device for input handling
///
/// This device provides a character device interface for reading input events.
/// Events are buffered in a ring buffer and can be read by user space through
/// standard read operations on the device file (e.g., /dev/input0).
pub struct EventDevice {
    /// Device name (e.g., "input0")
    name: String,
    /// Event queue (ring buffer)
    queue: Mutex<VecDeque<InputEvent>>,
    /// Waker for blocking reads
    waker: Waker,
    /// Non-blocking mode flag
    nonblocking: Mutex<bool>,
}

impl EventDevice {
    /// Create a new event device
    ///
    /// # Arguments
    ///
    /// * `device_type` - Device type ("keyboard", "mouse", "tablet", or "input")
    ///
    /// # Examples
    ///
    /// ```
    /// let event_dev = Arc::new(EventDevice::new("keyboard"));
    /// DeviceManager::get_mut_manager().register_device(event_dev);
    /// ```
    pub fn new(device_type: &str) -> Self {
        // Get incremented ID based on device type
        let id = match device_type {
            "keyboard" => KEYBOARD_COUNTER.fetch_add(1, Ordering::SeqCst),
            "mouse" => MOUSE_COUNTER.fetch_add(1, Ordering::SeqCst),
            "tablet" => TABLET_COUNTER.fetch_add(1, Ordering::SeqCst),
            _ => INPUT_COUNTER.fetch_add(1, Ordering::SeqCst),
        };

        // Generate device name
        let name = alloc::format!("{}{}", device_type, id);

        // Create a unique waker name based on the device name
        let waker_name = alloc::format!("event_{}", name).leak();

        Self {
            name,
            queue: Mutex::new(VecDeque::with_capacity(EVENT_QUEUE_CAPACITY)),
            waker: Waker::new_interruptible(waker_name),
            nonblocking: Mutex::new(false),
        }
    }

    /// Get the device name
    ///
    /// # Returns
    ///
    /// The device name (e.g., "keyboard0", "mouse0")
    pub fn get_name(&self) -> &str {
        &self.name
    }

    /// Push an input event into the queue
    ///
    /// This method should be called from interrupt handlers or device drivers
    /// when a new input event occurs. If the queue is full, the oldest event
    /// is dropped to make room for the new one.
    ///
    /// # Arguments
    ///
    /// * `type_` - Event type (EV_KEY, EV_REL, etc.)
    /// * `code` - Event code (KEY_A, REL_X, etc.)
    /// * `value` - Event value (1/0 for keys, movement delta, etc.)
    ///
    /// # Examples
    ///
    /// ```
    /// // In a mouse driver interrupt handler
    /// event_dev.push_event(EV_REL, REL_X, mouse_dx);
    /// event_dev.push_event(EV_REL, REL_Y, mouse_dy);
    /// event_dev.push_event(EV_SYN, SYN_REPORT, 0);
    /// ```
    pub fn push_event(&self, type_: u16, code: u16, value: i32) {
        let event = InputEvent::new(type_, code, value);

        {
            let mut q = self.queue.lock();

            // If queue is full, drop the oldest event
            if q.len() >= EVENT_QUEUE_CAPACITY {
                q.pop_front();
            }

            q.push_back(event);
        }

        // Wake up any waiting tasks
        self.waker.wake_one();
    }

    /// Check if there are events available to read
    fn has_events(&self) -> bool {
        !self.queue.lock().is_empty()
    }
}

// Implement Device trait for device registration
impl Device for EventDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "event_device"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

// Implement CharDevice trait for character device operations
impl CharDevice for EventDevice {
    fn read_byte(&self) -> Option<u8> {
        // Not suitable for byte-by-byte reading.
        // Events should be read as complete InputEvent structures.
        None
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        let event_size = InputEvent::size();

        // Buffer must be large enough to hold at least one event
        if buffer.len() < event_size {
            return 0;
        }

        // Try to read one event
        {
            let mut q = self.queue.lock();
            if let Some(event) = q.pop_front() {
                // Convert event structure to bytes
                let bytes = unsafe {
                    core::slice::from_raw_parts(&event as *const _ as *const u8, event_size)
                };

                buffer[..event_size].copy_from_slice(bytes);
                return event_size;
            }
        }

        // No data available - check nonblocking mode
        if *self.nonblocking.lock() {
            // In non-blocking mode, return immediately with 0 bytes
            return 0;
        }

        // In blocking mode, wait for data using waker
        use crate::task::mytask;

        if let Some(task) = mytask() {
            self.waker.wait(task.get_id(), task.get_trapframe());

            // After waking up, try to read again
            let mut q = self.queue.lock();
            if let Some(event) = q.pop_front() {
                let bytes = unsafe {
                    core::slice::from_raw_parts(&event as *const _ as *const u8, event_size)
                };
                buffer[..event_size].copy_from_slice(bytes);
                return event_size;
            }
        }

        // No task context or spurious wakeup
        0
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        // Delegate to read() which implements blocking
        Ok(self.read(buffer))
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("Write not supported on input event device")
    }

    fn can_read(&self) -> bool {
        self.has_events()
    }

    fn can_write(&self) -> bool {
        false
    }
}

// Implement Selectable trait for poll/select support
impl Selectable for EventDevice {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();

        if interest.read {
            set.read = self.has_events();
        }

        // Write is never ready for input devices
        set.write = false;

        set
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        timeout_ticks: Option<u64>,
    ) -> SelectWaitOutcome {
        // Only support read interest
        if !interest.read {
            return SelectWaitOutcome::Ready;
        }

        // If data is already available, return immediately
        if self.has_events() {
            return SelectWaitOutcome::Ready;
        }

        // If in non-blocking mode, don't wait
        if *self.nonblocking.lock() {
            return SelectWaitOutcome::Ready;
        }

        // Block until data arrives or timeout
        // Note: Current implementation doesn't support blocking in this context
        // because Waker::wait() requires task context that we don't have here.
        // For proper blocking behavior, the VFS layer should handle this.
        // For now, return Ready to avoid blocking the caller.
        if timeout_ticks.is_some() {
            // TODO: Implement timeout support when Waker supports it
            SelectWaitOutcome::TimedOut
        } else {
            // TODO: Properly implement blocking wait
            // self.waker.wait() requires current_task_id and trapframe
            // which are not available in this context.
            // For now, return Ready immediately.
            SelectWaitOutcome::Ready
        }
    }

    fn set_nonblocking(&self, enabled: bool) {
        *self.nonblocking.lock() = enabled;
    }

    fn is_nonblocking(&self) -> bool {
        *self.nonblocking.lock()
    }
}

// Implement required trait bounds for Device
impl ControlOps for EventDevice {}
impl MemoryMappingOps for EventDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for input event devices")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::input::event_types::*;
    use crate::device::input::rel_codes::*;
    use alloc::string::ToString;

    #[test_case]
    fn test_event_device_creation() {
        let dev = EventDevice::new("input");
        // Device name should be "input" + counter (e.g., "input0", "input1", etc.)
        assert!(dev.name.starts_with("input"));
        assert!(!dev.has_events());
    }

    #[test_case]
    fn test_push_and_read_event() {
        let dev = EventDevice::new("input");

        // Push an event
        dev.push_event(EV_REL, REL_X, 10);

        // Should have events now
        assert!(dev.has_events());

        // Read the event
        let mut buffer = [0u8; InputEvent::size()];
        let bytes_read = dev.read_at(0, &mut buffer).unwrap();

        assert_eq!(bytes_read, InputEvent::size());

        let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };

        assert_eq!(event.type_, EV_REL);
        assert_eq!(event.code, REL_X);
        assert_eq!(event.value, 10);

        // Should have no more events
        assert!(!dev.has_events());
    }

    #[test_case]
    fn test_queue_overflow() {
        let dev = EventDevice::new("input");

        // Fill the queue beyond capacity
        for i in 0..(EVENT_QUEUE_CAPACITY + 10) {
            dev.push_event(EV_KEY, 0, i as i32);
        }

        // Queue should be at capacity
        assert_eq!(dev.queue.lock().len(), EVENT_QUEUE_CAPACITY);

        // Read first event - should be event #10 (first 10 were dropped)
        let mut buffer = [0u8; InputEvent::size()];
        dev.read_at(0, &mut buffer).unwrap();

        let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };

        assert_eq!(event.value, 10);
    }
}
