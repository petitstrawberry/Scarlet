//! Event device implementation for input events
//!
//! This module provides an event device that implements the CharDevice trait
//! for handling input events from keyboards, mice, touchscreens, etc.

extern crate alloc;

use crate::sync::IrqSpinLock;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Static counters for device naming
static KEYBOARD_COUNTER: AtomicUsize = AtomicUsize::new(0);
static MOUSE_COUNTER: AtomicUsize = AtomicUsize::new(0);
static TOUCHPAD_COUNTER: AtomicUsize = AtomicUsize::new(0);
static TABLET_COUNTER: AtomicUsize = AtomicUsize::new(0);
static TOUCHSCREEN_COUNTER: AtomicUsize = AtomicUsize::new(0);
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

/// Return the input device's [`InputDeviceKind`].
pub const SCTL_INPUT_GET_KIND: u32 = 0x5353_0100;
/// Return the input device's capability bit mask.
pub const SCTL_INPUT_GET_CAPABILITIES: u32 = 0x5353_0101;
/// Return the minimum value for the absolute axis passed in `arg`.
pub const SCTL_INPUT_GET_ABS_MIN: u32 = 0x5353_0102;
/// Return the maximum value for the absolute axis passed in `arg`.
pub const SCTL_INPUT_GET_ABS_MAX: u32 = 0x5353_0103;

/// Device produces key or button events.
pub const INPUT_CAP_KEY: u32 = 1 << 0;
/// Device produces relative-axis events.
pub const INPUT_CAP_REL: u32 = 1 << 1;
/// Device produces absolute-axis events.
pub const INPUT_CAP_ABS: u32 = 1 << 2;
/// Device represents direct touch rather than an indirect pointer surface.
pub const INPUT_CAP_DIRECT_TOUCH: u32 = 1 << 3;

/// Largest Linux-compatible absolute-axis code accepted by the metadata ABI.
pub const ABS_MAX: u16 = 0x3f;

/// Stable input device classes exposed through `SCTL_INPUT_GET_KIND`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDeviceKind {
    /// Device class is not declared.
    Unknown = 0,
    /// Keyboard device.
    Keyboard = 1,
    /// Relative mouse device.
    Mouse = 2,
    /// Indirect touchpad device.
    Touchpad = 3,
    /// Direct touchscreen device.
    Touchscreen = 4,
    /// Graphics tablet or stylus device.
    Tablet = 5,
}

impl InputDeviceKind {
    fn from_device_type(device_type: &str) -> Self {
        match device_type {
            "keyboard" => Self::Keyboard,
            "mouse" => Self::Mouse,
            "touchpad" | "trackpad" => Self::Touchpad,
            "touchscreen" => Self::Touchscreen,
            "tablet" => Self::Tablet,
            _ => Self::Unknown,
        }
    }
}

/// Inclusive raw range for one `EV_ABS` axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AbsoluteAxisInfo {
    /// Linux-compatible absolute-axis code.
    pub code: u16,
    /// Inclusive logical minimum emitted by the device.
    pub minimum: i32,
    /// Inclusive logical maximum emitted by the device.
    pub maximum: i32,
}

/// Optional device classification and absolute-axis description.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDeviceMetadata {
    kind: InputDeviceKind,
    capabilities: u32,
    absolute_axes: Vec<AbsoluteAxisInfo>,
}

impl InputDeviceMetadata {
    /// Create empty metadata for a device class.
    ///
    /// # Arguments
    ///
    /// * `kind` - Logical input device class.
    /// * `capabilities` - `INPUT_CAP_*` bit mask.
    ///
    /// # Returns
    ///
    /// Metadata with no absolute axes declared.
    pub const fn new(kind: InputDeviceKind, capabilities: u32) -> Self {
        Self {
            kind,
            capabilities,
            absolute_axes: Vec::new(),
        }
    }

    /// Add a validated absolute-axis range.
    ///
    /// # Arguments
    ///
    /// * `code` - Linux-compatible `ABS_*` axis code.
    /// * `minimum` - Inclusive logical minimum.
    /// * `maximum` - Inclusive logical maximum, greater than `minimum`.
    ///
    /// # Returns
    ///
    /// Updated metadata, or an error for an invalid or duplicate axis.
    pub fn with_absolute_axis(
        mut self,
        code: u16,
        minimum: i32,
        maximum: i32,
    ) -> Result<Self, &'static str> {
        if code > ABS_MAX {
            return Err("Absolute axis code is out of range");
        }
        if minimum >= maximum {
            return Err("Absolute axis minimum must be less than maximum");
        }
        if self.absolute_axes.iter().any(|axis| axis.code == code) {
            return Err("Absolute axis metadata is duplicated");
        }
        self.absolute_axes.push(AbsoluteAxisInfo {
            code,
            minimum,
            maximum,
        });
        self.capabilities |= INPUT_CAP_ABS;
        Ok(self)
    }

    fn axis(&self, code: u16) -> Option<AbsoluteAxisInfo> {
        self.absolute_axes
            .iter()
            .copied()
            .find(|axis| axis.code == code)
    }
}

/// Event device for input handling
///
/// This device provides a character device interface for reading input events.
/// Events are buffered in a ring buffer and can be read by user space through
/// standard read operations on the device file (e.g., /dev/input0).
pub struct EventDevice {
    /// Device name (e.g., "input0")
    name: String,
    /// Optional classification and axis metadata.
    metadata: InputDeviceMetadata,
    /// Event queue (ring buffer)
    queue: IrqSpinLock<VecDeque<InputEvent>>,
    /// Waker for blocking reads
    waker: Waker,
    /// Non-blocking mode flag
    nonblocking: IrqSpinLock<bool>,
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
    /// DeviceManager::get_manager().register_device(event_dev);
    /// ```
    pub fn new(device_type: &str) -> Self {
        Self::new_with_metadata(
            device_type,
            InputDeviceMetadata::new(InputDeviceKind::from_device_type(device_type), 0),
        )
    }

    /// Create an event device with queryable input metadata.
    ///
    /// # Arguments
    ///
    /// * `device_type` - Prefix used for the registered device name.
    /// * `metadata` - Device class, capabilities, and absolute-axis ranges.
    ///
    /// # Returns
    ///
    /// A new event device carrying the supplied metadata.
    pub fn new_with_metadata(device_type: &str, metadata: InputDeviceMetadata) -> Self {
        // Get incremented ID based on device type
        let id = match device_type {
            "keyboard" => KEYBOARD_COUNTER.fetch_add(1, Ordering::SeqCst),
            "mouse" => MOUSE_COUNTER.fetch_add(1, Ordering::SeqCst),
            "touchpad" | "trackpad" => TOUCHPAD_COUNTER.fetch_add(1, Ordering::SeqCst),
            "tablet" => TABLET_COUNTER.fetch_add(1, Ordering::SeqCst),
            "touchscreen" => TOUCHSCREEN_COUNTER.fetch_add(1, Ordering::SeqCst),
            _ => INPUT_COUNTER.fetch_add(1, Ordering::SeqCst),
        };

        // Generate device name
        let name = alloc::format!("{}{}", device_type, id);

        // Create a unique waker name based on the device name
        let waker_name = alloc::format!("event_{}", name).leak();

        Self {
            name,
            metadata,
            queue: IrqSpinLock::new(VecDeque::with_capacity(EVENT_QUEUE_CAPACITY)),
            waker: Waker::new_interruptible(waker_name),
            nonblocking: IrqSpinLock::new(false),
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

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
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
        _min_wait_ticks: u64,
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
impl ControlOps for EventDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        const SCTL_SET_NONBLOCKING: u32 = 0x5353_0007;
        match command {
            SCTL_SET_NONBLOCKING => {
                self.set_nonblocking(arg != 0);
                Ok(0)
            }
            SCTL_INPUT_GET_KIND => Ok(i32::from(self.metadata.kind as u8)),
            SCTL_INPUT_GET_CAPABILITIES => i32::try_from(self.metadata.capabilities)
                .map_err(|_| "Input capability mask exceeds control return range"),
            SCTL_INPUT_GET_ABS_MIN | SCTL_INPUT_GET_ABS_MAX => {
                let code = u16::try_from(arg).map_err(|_| "Invalid absolute axis code")?;
                if code > ABS_MAX {
                    return Err("Invalid absolute axis code");
                }
                let axis = self
                    .metadata
                    .axis(code)
                    .ok_or("Absolute axis is not supported")?;
                if command == SCTL_INPUT_GET_ABS_MIN {
                    Ok(axis.minimum)
                } else {
                    Ok(axis.maximum)
                }
            }
            _ => Err("Control operation not supported"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (SCTL_INPUT_GET_KIND, "Get input device kind"),
            (SCTL_INPUT_GET_CAPABILITIES, "Get input device capabilities",),
            (SCTL_INPUT_GET_ABS_MIN, "Get absolute axis minimum"),
            (SCTL_INPUT_GET_ABS_MAX, "Get absolute axis maximum"),
        ]
    }
}
impl MemoryMappingOps for EventDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported for input event devices")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::input::event_types::*;
    use crate::device::input::rel_codes::*;

    #[test_case]
    fn test_event_device_creation() {
        let dev = EventDevice::new("input");
        // Device name should be "input" + counter (e.g., "input0", "input1", etc.)
        assert!(dev.name.starts_with("input"));
        assert!(!dev.has_events());
    }

    #[test_case]
    fn test_default_metadata_preserves_device_kind_without_axes() {
        let dev = EventDevice::new("tablet");
        assert_eq!(
            dev.control(SCTL_INPUT_GET_KIND, 0).unwrap(),
            InputDeviceKind::Tablet as i32
        );
        assert_eq!(dev.control(SCTL_INPUT_GET_CAPABILITIES, 0).unwrap(), 0);
        assert!(dev.control(SCTL_INPUT_GET_ABS_MIN, 0).is_err());

        let touchpad = EventDevice::new("touchpad");
        assert!(touchpad.name.starts_with("touchpad"));
        assert_eq!(
            touchpad.control(SCTL_INPUT_GET_KIND, 0).unwrap(),
            InputDeviceKind::Touchpad as i32
        );
    }

    #[test_case]
    fn test_absolute_axis_metadata_control_queries() {
        let metadata = InputDeviceMetadata::new(
            InputDeviceKind::Touchscreen,
            INPUT_CAP_KEY | INPUT_CAP_DIRECT_TOUCH,
        )
        .with_absolute_axis(0x00, 0, 4095)
        .unwrap()
        .with_absolute_axis(0x01, 0, 2047)
        .unwrap();
        let dev = EventDevice::new_with_metadata("touchscreen", metadata);

        assert!(dev.name.starts_with("touchscreen"));
        assert_eq!(
            dev.control(SCTL_INPUT_GET_KIND, 0).unwrap(),
            InputDeviceKind::Touchscreen as i32
        );
        assert_eq!(
            dev.control(SCTL_INPUT_GET_CAPABILITIES, 0).unwrap(),
            (INPUT_CAP_KEY | INPUT_CAP_ABS | INPUT_CAP_DIRECT_TOUCH) as i32
        );
        assert_eq!(dev.control(SCTL_INPUT_GET_ABS_MIN, 0x00).unwrap(), 0);
        assert_eq!(dev.control(SCTL_INPUT_GET_ABS_MAX, 0x00).unwrap(), 4095);
        assert_eq!(dev.control(SCTL_INPUT_GET_ABS_MAX, 0x01).unwrap(), 2047);
        assert!(dev.control(SCTL_INPUT_GET_ABS_MAX, usize::MAX).is_err());
        assert!(dev.control(SCTL_INPUT_GET_ABS_MAX, 0x18).is_err());
    }

    #[test_case]
    fn test_absolute_axis_metadata_rejects_invalid_ranges() {
        let metadata = InputDeviceMetadata::new(InputDeviceKind::Touchscreen, 0)
            .with_absolute_axis(0x00, 10, 10);
        assert!(metadata.is_err());

        let metadata = InputDeviceMetadata::new(InputDeviceKind::Touchscreen, 0)
            .with_absolute_axis(0x00, 0, 10)
            .unwrap();
        assert!(metadata.with_absolute_axis(0x00, 0, 20).is_err());
        assert!(
            InputDeviceMetadata::new(InputDeviceKind::Touchscreen, 0)
                .with_absolute_axis(ABS_MAX + 1, 0, 10)
                .is_err()
        );
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
