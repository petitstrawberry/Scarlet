//! Event device implementation for input events
//!
//! This module provides an event device that implements the CharDevice trait
//! for handling input events from keyboards, mice, touchscreens, etc.

extern crate alloc;

mod snapshots;

use crate::sync::IrqSpinLock;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Static counters for device naming
static KEYBOARD_COUNTER: AtomicUsize = AtomicUsize::new(0);
static MOUSE_COUNTER: AtomicUsize = AtomicUsize::new(0);
static TOUCHPAD_COUNTER: AtomicUsize = AtomicUsize::new(0);
static TABLET_COUNTER: AtomicUsize = AtomicUsize::new(0);
static TOUCHSCREEN_COUNTER: AtomicUsize = AtomicUsize::new(0);
static SWITCH_COUNTER: AtomicUsize = AtomicUsize::new(0);
static BUTTONS_COUNTER: AtomicUsize = AtomicUsize::new(0);
static INPUT_COUNTER: AtomicUsize = AtomicUsize::new(0);

// SWS probes gamepad0 through gamepad7. All native gamepad constructors
// reserve from the same pool, including the permanent rail controller.
const GAMEPAD_SLOTS: usize = 8;
type Slots = IrqSpinLock<[bool; GAMEPAD_SLOTS]>;
static USED_GAMEPADS: Slots = Slots::new([false; GAMEPAD_SLOTS]);
static LEFT_JOYCONS: Slots = Slots::new([false; GAMEPAD_SLOTS]);
static RIGHT_JOYCONS: Slots = Slots::new([false; GAMEPAD_SLOTS]);
// Preserve the infallible constructor for permanent devices beyond the scan range.
static EXTRA_GAMEPAD_COUNTER: AtomicUsize = AtomicUsize::new(GAMEPAD_SLOTS);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoyConSide {
    Left,
    Right,
}

struct GamepadSlot {
    index: usize,
    pool: &'static Slots,
}

impl GamepadSlot {
    fn reserve(pool: &'static Slots) -> Option<Self> {
        let mut used = pool.lock();
        let index = used.iter().position(|&occupied| !occupied)?;
        used[index] = true;
        Some(Self { index, pool })
    }
}

impl Drop for GamepadSlot {
    fn drop(&mut self) {
        self.pool.lock()[self.index] = false;
    }
}

enum Attachment {
    Permanent { _slot: Option<GamepadSlot> },
    Removable { _slot: GamepadSlot },
    Detached,
}

use crate::arch::Trapframe;
use crate::device::char::CharDevice;
use crate::device::{Device, DeviceType};
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::object::capability::{ControlOps, MemoryMappingOps, StreamError};
use crate::sync::Waker;

use super::InputEvent;
use super::event_types::EV_SW;
use super::switch_codes::SW_MAX;
use super::syn_codes::{SYN_DROPPED, SYN_REPORT};

/// Maximum number of events to buffer
const EVENT_QUEUE_CAPACITY: usize = 256;

/// Buffered events together with recovery state after an overflow.
struct EventQueue {
    events: VecDeque<InputEvent>,
    attachment: Attachment,
    history: Option<snapshots::History>,
    /// Drop the remainder of the interrupted input frame before accepting a new one.
    discard_until_syn_report: bool,
}

impl EventQueue {
    fn new(attachment: Attachment) -> Self {
        Self {
            events: VecDeque::with_capacity(EVENT_QUEUE_CAPACITY),
            attachment,
            history: None,
            discard_until_syn_report: false,
        }
    }

    fn detached(&self) -> bool {
        matches!(self.attachment, Attachment::Detached)
    }

    fn mark_overflow(&mut self, time: u64) {
        self.events.clear();
        for code in [SYN_DROPPED, SYN_REPORT] {
            self.events.push_back(InputEvent {
                time,
                type_: super::event_types::EV_SYN,
                code,
                value: 0,
            });
        }
    }
}

/// Return the input device's [`InputDeviceKind`].
pub const SCTL_INPUT_GET_KIND: u32 = 0x5353_0100;
/// Return the input device's capability bit mask.
pub const SCTL_INPUT_GET_CAPABILITIES: u32 = 0x5353_0101;
/// Return the minimum value for the absolute axis passed in `arg`.
pub const SCTL_INPUT_GET_ABS_MIN: u32 = 0x5353_0102;
/// Return the maximum value for the absolute axis passed in `arg`.
pub const SCTL_INPUT_GET_ABS_MAX: u32 = 0x5353_0103;
/// Return the number of concurrently reportable multitouch contacts.
pub const SCTL_INPUT_GET_MT_SLOT_COUNT: u32 = 0x5353_0104;
/// Return the bit mask of supported `SW_*` switch codes.
pub const SCTL_INPUT_GET_SWITCH_CAPABILITIES: u32 = 0x5353_0105;
/// Return the current state of the `SW_*` switch code passed in `arg`.
pub const SCTL_INPUT_GET_SWITCH_STATE: u32 = 0x5353_0106;
/// Query support for packed (event type << 16 | code), returning zero or one.
pub const SCTL_INPUT_HAS_EVENT_CODE: u32 = 0x5353_0107;
/// Query current value for a supported packed (event type << 16 | code).
pub const SCTL_INPUT_GET_EVENT_VALUE: u32 = 0x5353_0108;

/// Device produces key or button events.
pub const INPUT_CAP_KEY: u32 = 1 << 0;
/// Device produces relative-axis events.
pub const INPUT_CAP_REL: u32 = 1 << 1;
/// Device produces absolute-axis events.
pub const INPUT_CAP_ABS: u32 = 1 << 2;
/// Device represents direct touch rather than an indirect pointer surface.
pub const INPUT_CAP_DIRECT_TOUCH: u32 = 1 << 3;
/// Device reports multitouch slots and contact axes.
pub const INPUT_CAP_MT: u32 = 1 << 4;
/// Device reports `EV_SW` switch state changes.
pub const INPUT_CAP_SWITCH: u32 = 1 << 5;
/// Device is physically integrated into the system rather than externally attached.
pub const INPUT_CAP_INTERNAL: u32 = 1 << 6;

/// Largest Linux-compatible absolute-axis code accepted by the metadata ABI.
pub const ABS_MAX: u16 = 0x3f;
/// Largest multitouch slot table accepted from a kernel input driver.
///
/// Keeping this bounded prevents malformed metadata from forcing userspace to
/// allocate an unreasonably large per-device slot array.
pub const MAX_MT_SLOTS: usize = 64;

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
    /// Posture, lid, or other switch device.
    Switch = 6,
    /// Game controller with buttons and absolute stick or hat axes.
    Gamepad = 7,
    /// System buttons without a text keyboard (for example volume keys).
    Buttons = 8,
}

impl InputDeviceKind {
    fn from_device_type(device_type: &str) -> Self {
        match device_type {
            "keyboard" => Self::Keyboard,
            "mouse" => Self::Mouse,
            "touchpad" | "trackpad" => Self::Touchpad,
            "touchscreen" => Self::Touchscreen,
            "tablet" => Self::Tablet,
            "switch" => Self::Switch,
            "gamepad" => Self::Gamepad,
            "buttons" => Self::Buttons,
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

/// Optional device classification, multitouch, switch, and absolute-axis description.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDeviceMetadata {
    kind: InputDeviceKind,
    capabilities: u32,
    absolute_axes: Vec<AbsoluteAxisInfo>,
    gamepad_buttons: Option<Vec<u16>>,
    multitouch_slot_count: Option<u16>,
    switch_capabilities: u32,
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
            gamepad_buttons: None,
            multitouch_slot_count: None,
            switch_capabilities: 0,
        }
    }

    pub(super) fn with_gamepad_buttons(mut self, buttons: &[u16]) -> Self {
        self.gamepad_buttons = Some(buttons.to_vec());
        self
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

    /// Declare the number of concurrently reportable multitouch contacts.
    ///
    /// # Arguments
    ///
    /// * `slot_count` - Number of contacts addressable by `ABS_MT_SLOT`.
    ///
    /// # Returns
    ///
    /// Updated metadata, or an error when the count cannot be represented by
    /// the control ABI.
    pub fn with_multitouch_slots(mut self, slot_count: usize) -> Result<Self, &'static str> {
        if slot_count == 0 || slot_count > MAX_MT_SLOTS {
            return Err("Multitouch slot count is out of range");
        }
        if self.multitouch_slot_count.is_some() {
            return Err("Multitouch slot metadata is duplicated");
        }
        self.multitouch_slot_count =
            Some(u16::try_from(slot_count).map_err(|_| "Multitouch slot count is out of range")?);
        self.capabilities |= INPUT_CAP_MT;
        Ok(self)
    }

    /// Declare a supported posture or lid switch code.
    ///
    /// # Arguments
    ///
    /// * `code` - Linux-compatible `SW_*` switch code.
    ///
    /// # Returns
    ///
    /// Updated metadata, or an error for an invalid or duplicate switch code.
    pub fn with_switch(mut self, code: u16) -> Result<Self, &'static str> {
        if code > SW_MAX {
            return Err("Switch code is out of range");
        }
        let bit = 1_u32 << u32::from(code);
        if self.switch_capabilities & bit != 0 {
            return Err("Switch capability metadata is duplicated");
        }
        self.switch_capabilities |= bit;
        self.capabilities |= INPUT_CAP_SWITCH;
        Ok(self)
    }

    fn axis(&self, code: u16) -> Option<AbsoluteAxisInfo> {
        self.absolute_axes
            .iter()
            .copied()
            .find(|axis| axis.code == code)
    }

    fn supports_switch(&self, code: u16) -> bool {
        code <= SW_MAX && self.switch_capabilities & (1_u32 << u32::from(code)) != 0
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
    /// Current state of declared Linux-compatible `SW_*` switch codes.
    switch_state: IrqSpinLock<u32>,
    /// Event queue and frame-recovery state.
    queue: IrqSpinLock<EventQueue>,
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
        let mut slot = None;
        let id = match device_type {
            "keyboard" => KEYBOARD_COUNTER.fetch_add(1, Ordering::SeqCst),
            "mouse" => MOUSE_COUNTER.fetch_add(1, Ordering::SeqCst),
            "touchpad" | "trackpad" => TOUCHPAD_COUNTER.fetch_add(1, Ordering::SeqCst),
            "tablet" => TABLET_COUNTER.fetch_add(1, Ordering::SeqCst),
            "touchscreen" => TOUCHSCREEN_COUNTER.fetch_add(1, Ordering::SeqCst),
            "switch" => SWITCH_COUNTER.fetch_add(1, Ordering::SeqCst),
            "gamepad" => {
                slot = GamepadSlot::reserve(&USED_GAMEPADS);
                slot.as_ref().map_or_else(
                    || EXTRA_GAMEPAD_COUNTER.fetch_add(1, Ordering::Relaxed),
                    |s| s.index,
                )
            }
            "buttons" => BUTTONS_COUNTER.fetch_add(1, Ordering::SeqCst),
            _ => INPUT_COUNTER.fetch_add(1, Ordering::SeqCst),
        };

        Self::named(
            alloc::format!("{}{}", device_type, id),
            metadata,
            Attachment::Permanent { _slot: slot },
        )
    }

    /// Reserve a discoverable gamepad name before publishing the endpoint.
    pub(crate) fn new_removable_gamepad(
        metadata: InputDeviceMetadata,
    ) -> Result<Self, &'static str> {
        Self::removable("gamepad", &USED_GAMEPADS, metadata).map(|(_, device)| device)
    }

    pub(super) fn logical_gamepad(
        metadata: InputDeviceMetadata,
    ) -> Result<(usize, Self), &'static str> {
        let (slot, device) = Self::removable("gamepad", &USED_GAMEPADS, metadata)?;
        device.queue.lock().history = Some(snapshots::History::default());
        Ok((slot, device))
    }

    fn removable(
        prefix: &str,
        pool: &'static Slots,
        metadata: InputDeviceMetadata,
    ) -> Result<(usize, Self), &'static str> {
        let slot =
            GamepadSlot::reserve(pool).ok_or("All discoverable slots of this type are occupied")?;
        let index = slot.index;
        Ok((
            index,
            Self::named(
                alloc::format!("{prefix}{index}"),
                metadata,
                Attachment::Removable { _slot: slot },
            ),
        ))
    }

    /// Publish a physical half under its side-specific name. Seed a complete
    /// input snapshot before registration so even the first open has state.
    pub fn new_removable_joycon(
        side: JoyConSide,
        metadata: InputDeviceMetadata,
    ) -> Result<Self, &'static str> {
        if metadata.kind != InputDeviceKind::Gamepad {
            return Err("Raw Joy-Con requires gamepad metadata");
        }
        let (prefix, pool) = match side {
            JoyConSide::Left => ("joycon-left", &LEFT_JOYCONS),
            JoyConSide::Right => ("joycon-right", &RIGHT_JOYCONS),
        };
        let (_, device) = Self::removable(prefix, pool, metadata)?;
        device.queue.lock().history = Some(snapshots::History::joycon());
        Ok(device)
    }

    /// Take the newest pending player number for this physical endpoint.
    pub fn take_joycon_player(&self) -> Option<u8> {
        let mut q = self.queue.lock();
        if q.detached() {
            return None;
        }
        q.history.as_mut()?.player.as_mut()?.pending.take()
    }

    /// Install the physical producer's notification before publishing its node.
    /// The callback can take the pending assignment or wake a sleeping worker.
    pub fn set_joycon_player_notify(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        let (previous, pending) = {
            let mut q = self.queue.lock();
            if q.detached() {
                return;
            }
            let Some(player) = q.history.as_mut().and_then(|h| h.player.as_mut()) else {
                return;
            };
            (
                player.notify.replace(notify.clone()),
                player.pending.is_some(),
            )
        };
        drop(previous);
        if pending {
            notify();
        }
    }

    fn named(name: String, metadata: InputDeviceMetadata, attachment: Attachment) -> Self {
        // Waiter identity belongs to the Waker, not this diagnostic label.
        // Virtual channels can be short-lived, so do not leak a label per device.
        let waker_name = "input-event";

        Self {
            name,
            metadata,
            switch_state: IrqSpinLock::new(0),
            queue: IrqSpinLock::new(EventQueue::new(attachment)),
            waker: Waker::new_interruptible(waker_name),
            nonblocking: IrqSpinLock::new(false),
        }
    }

    /// Called by the producer after its final frame and registry removal.
    /// Open descriptors retain this queue while a new producer can reuse its name.
    pub fn detach_gamepad(&self) {
        let notify = {
            let mut queue = self.queue.lock();
            if !matches!(queue.attachment, Attachment::Removable { .. }) {
                return;
            }
            queue.attachment = Attachment::Detached;
            queue.history.as_mut().and_then(|h| h.player.take())
        };
        drop(notify);
        self.waker.wake_all();
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
    /// when a new input event occurs. If the queue is full, queued events and
    /// the interrupted frame are discarded. Readers then receive
    /// `SYN_DROPPED`, followed by `SYN_REPORT`, before the next complete frame.
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
        self.push_events(&[(type_, code, value)]);
    }

    /// Publish a batch with one timestamp, queue lock, and reader wakeup.
    /// Callers may pass a complete frame ending in `SYN_REPORT` so no reader
    /// can observe a partial frame before the producer has finished it.
    pub fn push_events(&self, events: &[(u16, u16, i32)]) {
        if events.is_empty() {
            return;
        }
        for &(type_, code, value) in events {
            if type_ == EV_SW && code <= SW_MAX {
                let bit = 1_u32 << u32::from(code);
                let mut switch_state = self.switch_state.lock();
                if value == 0 {
                    *switch_state &= !bit;
                } else {
                    *switch_state |= bit;
                }
            }
        }

        let time = crate::time::current_time_ns();
        {
            let mut q = self.queue.lock();
            if q.detached() {
                return;
            }
            if q.history.is_some() {
                if q.push_snapshot(events, time) {
                    drop(q);
                    self.waker.wake_all();
                }
                return;
            }
            // Joy-Con batches are complete state snapshots. Make room for the
            // whole snapshot, especially the final neutral frame on unplug.
            if matches!(q.attachment, Attachment::Removable { .. })
                && events.last() == Some(&(super::event_types::EV_SYN, SYN_REPORT, 0))
                && events.len() <= EVENT_QUEUE_CAPACITY - 2
                && (q.discard_until_syn_report
                    || q.events.len() + events.len() > EVENT_QUEUE_CAPACITY)
            {
                q.mark_overflow(time);
                q.discard_until_syn_report = false;
            }
            for &(type_, code, value) in events {
                if q.discard_until_syn_report {
                    if type_ == super::event_types::EV_SYN && code == SYN_REPORT {
                        q.discard_until_syn_report = false;
                    }
                } else if q.events.len() >= EVENT_QUEUE_CAPACITY {
                    // Discard the interrupted frame through its boundary so
                    // the next accepted event begins a complete frame.
                    q.mark_overflow(time);
                    q.discard_until_syn_report = true;
                } else {
                    q.events.push_back(InputEvent {
                        time,
                        type_,
                        code,
                        value,
                    });
                }
            }
        }
        self.waker.wake_one();
    }

    fn read_ready(&self) -> bool {
        let q = self.queue.lock();
        !q.events.is_empty() || q.detached()
    }

    /// Drain complete native records without sleeping or changing the device's
    /// shared nonblocking flag. An empty queue returns zero.
    pub(super) fn drain_records(&self, buffer: &mut [u8]) -> usize {
        let mut queue = self.queue.lock();
        if queue.history.is_some() {
            // Snapshot history is consumed exclusively through per-open cursors.
            return 0;
        }
        let mut count = 0;
        for record in buffer.chunks_exact_mut(InputEvent::size()) {
            let Some(event) = queue.events.pop_front() else {
                break;
            };
            encode_event(event, record);
            count += InputEvent::size();
        }
        count
    }
}

fn encode_event(event: InputEvent, record: &mut [u8]) {
    record[..8].copy_from_slice(&event.time.to_ne_bytes());
    record[8..10].copy_from_slice(&event.type_.to_ne_bytes());
    record[10..12].copy_from_slice(&event.code.to_ne_bytes());
    record[12..16].copy_from_slice(&event.value.to_ne_bytes());
}

// Implement Device trait for device registration
impl Device for EventDevice {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        let q = self.queue.lock();
        let removable = match q.attachment {
            Attachment::Permanent { .. } => false,
            Attachment::Removable { .. } => true,
            Attachment::Detached => return Err("Input device detached"),
        };
        if q.history.is_some() {
            return Ok(Arc::new(snapshots::Reader::new(self.clone(), &q)));
        }
        drop(q);
        if removable {
            // This descriptor observes hangup on its original queue even after
            // another gamepad takes its name.
            Ok(self)
        } else {
            Ok(Arc::new(crate::device::DefaultDeviceOpen::new(self)))
        }
    }

    fn capabilities(&self) -> &'static [crate::device::DeviceCapability] {
        &[crate::device::DeviceCapability::PhysicalSeat]
    }

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
        self.try_read(buffer).unwrap_or(0)
    }

    fn try_read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if buffer.len() < InputEvent::size() {
            return Ok(0);
        }
        let mut next = || {
            let mut q = self.queue.lock();
            if q.history.is_some() {
                return Some(Err(StreamError::NotSupported));
            }
            if let Some(event) = q.events.pop_front() {
                encode_event(event, &mut buffer[..InputEvent::size()]);
                Some(Ok(InputEvent::size()))
            } else if q.detached() {
                Some(Err(StreamError::EndOfStream))
            } else {
                None
            }
        };
        if let Some(result) = next() {
            return result;
        }
        if *self.nonblocking.lock() {
            return Ok(0);
        }
        if let Some(task) = crate::task::mytask() {
            // Recheck the queue *after* wait registration as well. Multiple
            // readers racing the final broadcast must all observe permanent EOF.
            self.waker
                .wait_with_condition(task.get_id(), task.get_trapframe(), None, 0, || {
                    self.read_ready()
                });
            return next().unwrap_or(Ok(0));
        }
        Ok(0)
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        Ok(self.read(buffer))
    }

    fn try_read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // DevFS forwards this structured result to native StreamRead. SWS
        // retries a short live read and releases its path claim on end-of-stream.
        self.try_read(buffer)
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("Write not supported on input event device")
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("Write not supported on input event device")
    }

    fn can_read(&self) -> bool {
        self.read_ready()
    }

    fn can_write(&self) -> bool {
        false
    }
}

// Implement Selectable trait for poll/select support
impl Selectable for EventDevice {
    fn is_hung_up(&self) -> bool {
        self.queue.lock().detached()
    }

    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();

        let q = self.queue.lock();
        set.read = interest.read && (!q.events.is_empty() || q.detached());
        set.except = interest.except && q.detached();

        // Write is never ready for input devices
        set.write = false;

        set
    }

    fn subscribe_readiness(
        &self,
        interest: ReadyInterest,
        waiter: &Arc<Waker>,
    ) -> Option<Vec<crate::sync::readiness::ReadinessRegistration>> {
        if interest.read || interest.except {
            Some(alloc::vec![self.waker.subscribe_readiness(waiter)])
        } else {
            // Input devices have no writable readiness source.
            Some(Vec::new())
        }
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
        if self.read_ready() {
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
            SCTL_INPUT_HAS_EVENT_CODE if self.metadata.gamepad_buttons.is_some() => {
                let packed = u32::try_from(arg).map_err(|_| "Invalid event selector")?;
                let code = packed as u16;
                Ok(i32::from(match packed >> 16 {
                    0 => code == SYN_REPORT,
                    1 => self
                        .metadata
                        .gamepad_buttons
                        .as_ref()
                        .is_some_and(|keys| keys.contains(&code)),
                    3 => self.metadata.axis(code).is_some(),
                    _ => false,
                }))
            }
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
            SCTL_INPUT_GET_MT_SLOT_COUNT => self
                .metadata
                .multitouch_slot_count
                .map(i32::from)
                .ok_or("Multitouch slot count is not declared"),
            SCTL_INPUT_GET_SWITCH_CAPABILITIES => i32::try_from(self.metadata.switch_capabilities)
                .map_err(|_| "Switch capability mask exceeds control return range"),
            SCTL_INPUT_GET_SWITCH_STATE => {
                let code = u16::try_from(arg).map_err(|_| "Invalid switch code")?;
                if !self.metadata.supports_switch(code) {
                    return Err("Switch is not supported");
                }
                let state = *self.switch_state.lock();
                Ok(i32::from(state & (1_u32 << u32::from(code)) != 0))
            }
            _ => Err("Control operation not supported"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        let mut commands = alloc::vec![
            (SCTL_INPUT_GET_KIND, "Get input device kind"),
            (SCTL_INPUT_GET_CAPABILITIES, "Get input device capabilities",),
            (SCTL_INPUT_GET_ABS_MIN, "Get absolute axis minimum"),
            (SCTL_INPUT_GET_ABS_MAX, "Get absolute axis maximum"),
            (SCTL_INPUT_GET_MT_SLOT_COUNT, "Get multitouch slot count"),
            (
                SCTL_INPUT_GET_SWITCH_CAPABILITIES,
                "Get switch capabilities"
            ),
            (SCTL_INPUT_GET_SWITCH_STATE, "Get switch state"),
        ];
        if self.metadata.gamepad_buttons.is_some() {
            commands.push((SCTL_INPUT_HAS_EVENT_CODE, "Query gamepad control support"));
        }
        commands
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
    use crate::device::input::abs_codes::*;
    use crate::device::input::event_types::*;
    use crate::device::input::rel_codes::*;
    use crate::device::input::switch_codes::*;

    #[test_case]
    fn test_multitouch_and_switch_constants_match_linux() {
        assert_eq!(EV_SW, 0x05);
        assert_eq!(crate::device::input::syn_codes::SYN_MT_REPORT, 0x02);
        assert_eq!(crate::device::input::syn_codes::SYN_DROPPED, 0x03);
        assert_eq!(ABS_MT_SLOT, 0x2f);
        assert_eq!(ABS_MT_TOUCH_MAJOR, 0x30);
        assert_eq!(ABS_MT_POSITION_X, 0x35);
        assert_eq!(ABS_MT_POSITION_Y, 0x36);
        assert_eq!(ABS_MT_TRACKING_ID, 0x39);
        assert_eq!(ABS_MT_PRESSURE, 0x3a);
        assert_eq!(SW_LID, 0x00);
        assert_eq!(SW_TABLET_MODE, 0x01);
    }

    #[test_case]
    fn test_event_device_creation() {
        let dev = EventDevice::new("input");
        // Device name should be "input" + counter (e.g., "input0", "input1", etc.)
        assert!(dev.name.starts_with("input"));
        assert!(!dev.read_ready());
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
    fn test_multitouch_slot_metadata_bounds_and_control() {
        assert!(
            InputDeviceMetadata::new(InputDeviceKind::Touchscreen, 0)
                .with_multitouch_slots(0)
                .is_err()
        );
        assert!(
            InputDeviceMetadata::new(InputDeviceKind::Touchscreen, 0)
                .with_multitouch_slots(MAX_MT_SLOTS + 1)
                .is_err()
        );

        let metadata = InputDeviceMetadata::new(InputDeviceKind::Touchscreen, 0)
            .with_multitouch_slots(MAX_MT_SLOTS)
            .unwrap();
        assert!(metadata.clone().with_multitouch_slots(5).is_err());
        let dev = EventDevice::new_with_metadata("touchscreen", metadata);
        assert_eq!(
            dev.control(SCTL_INPUT_GET_MT_SLOT_COUNT, 0).unwrap(),
            MAX_MT_SLOTS as i32
        );
        assert_eq!(
            dev.control(SCTL_INPUT_GET_CAPABILITIES, 0).unwrap(),
            INPUT_CAP_MT as i32
        );
    }

    #[test_case]
    fn test_switch_metadata_controls_and_event_state() {
        let metadata = InputDeviceMetadata::new(InputDeviceKind::Switch, INPUT_CAP_INTERNAL)
            .with_switch(SW_LID)
            .unwrap()
            .with_switch(SW_TABLET_MODE)
            .unwrap();
        let dev = EventDevice::new_with_metadata("switch", metadata);

        assert_eq!(
            dev.control(SCTL_INPUT_GET_KIND, 0).unwrap(),
            InputDeviceKind::Switch as i32
        );
        assert_eq!(
            dev.control(SCTL_INPUT_GET_CAPABILITIES, 0).unwrap(),
            (INPUT_CAP_INTERNAL | INPUT_CAP_SWITCH) as i32
        );
        assert_eq!(
            dev.control(SCTL_INPUT_GET_SWITCH_CAPABILITIES, 0).unwrap(),
            ((1 << SW_LID) | (1 << SW_TABLET_MODE)) as i32
        );
        assert_eq!(
            dev.control(SCTL_INPUT_GET_SWITCH_STATE, SW_LID as usize)
                .unwrap(),
            0
        );

        dev.push_event(EV_SW, SW_LID, 1);
        assert_eq!(
            dev.control(SCTL_INPUT_GET_SWITCH_STATE, SW_LID as usize)
                .unwrap(),
            1
        );
        dev.push_event(EV_SW, SW_LID, 0);
        assert_eq!(
            dev.control(SCTL_INPUT_GET_SWITCH_STATE, SW_LID as usize)
                .unwrap(),
            0
        );
        assert!(
            dev.control(SCTL_INPUT_GET_SWITCH_STATE, SW_MAX as usize + 1)
                .is_err()
        );
        assert!(
            InputDeviceMetadata::new(InputDeviceKind::Switch, 0)
                .with_switch(SW_LID)
                .unwrap()
                .with_switch(SW_LID)
                .is_err()
        );
    }

    #[test_case]
    fn test_push_and_read_event() {
        let dev = EventDevice::new("input");

        // Push an event
        dev.push_event(EV_REL, REL_X, 10);

        // Should have events now
        assert!(dev.read_ready());

        // Read the event
        let mut buffer = [0u8; InputEvent::size()];
        let bytes_read = dev.read_at(0, &mut buffer).unwrap();

        assert_eq!(bytes_read, InputEvent::size());

        let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };

        assert_eq!(event.type_, EV_REL);
        assert_eq!(event.code, REL_X);
        assert_eq!(event.value, 10);

        // Should have no more events
        assert!(!dev.read_ready());
    }

    #[test_case]
    fn test_queue_overflow() {
        let dev = EventDevice::new("input");

        // Fill the queue, then begin a multitouch frame that overflows before
        // it can be completed. The incomplete frame must never reach readers.
        for i in 0..EVENT_QUEUE_CAPACITY {
            dev.push_event(EV_KEY, 0, i as i32);
        }
        dev.push_event(super::super::event_types::EV_ABS, ABS_MT_SLOT, 0);

        let q = dev.queue.lock();
        assert_eq!(q.events.len(), 2);
        assert!(q.discard_until_syn_report);
        drop(q);

        // End the discarded frame, then emit a complete next frame.
        dev.push_event(super::super::event_types::EV_ABS, ABS_MT_TRACKING_ID, 7);
        dev.push_event(EV_SYN, SYN_REPORT, 0);
        dev.push_event(super::super::event_types::EV_ABS, ABS_MT_SLOT, 1);
        dev.push_event(super::super::event_types::EV_ABS, ABS_MT_TRACKING_ID, 8);
        dev.push_event(EV_SYN, SYN_REPORT, 0);

        // The overflow marker and its empty boundary precede the complete
        // post-overflow frame; no contact from the interrupted frame remains.
        let mut buffer = [0u8; InputEvent::size()];
        let mut events = [InputEvent::default(); 5];
        for event in &mut events {
            dev.read_at(0, &mut buffer).unwrap();
            *event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };
        }
        assert_eq!((events[0].type_, events[0].code), (EV_SYN, SYN_DROPPED));
        assert_eq!((events[1].type_, events[1].code), (EV_SYN, SYN_REPORT));
        assert_eq!(
            (events[2].type_, events[2].code, events[2].value),
            (super::super::event_types::EV_ABS, ABS_MT_SLOT, 1)
        );
        assert_eq!(
            (events[3].type_, events[3].code, events[3].value),
            (super::super::event_types::EV_ABS, ABS_MT_TRACKING_ID, 8)
        );
        assert_eq!((events[4].type_, events[4].code), (EV_SYN, SYN_REPORT));
    }

    #[test_case]
    fn gamepad_reconnect_reuses_only_unpublished_slots_while_old_files_are_open() {
        use crate::device::manager::DeviceManager;
        let manager = DeviceManager::new_for_test();
        let metadata = InputDeviceMetadata::new(InputDeviceKind::Gamepad, INPUT_CAP_KEY);
        let wired = EventDevice::new_with_metadata("gamepad", metadata.clone());
        let mut held = Vec::new();
        while let Ok(pad) = EventDevice::new_removable_gamepad(metadata.clone()) {
            held.push(pad);
            assert!(held.len() <= GAMEPAD_SLOTS);
        }
        let available = held.pop().unwrap();
        let name = available.name.clone();
        assert!(name["gamepad".len()..].parse::<usize>().unwrap() < GAMEPAD_SLOTS);
        assert_ne!(name, wired.name);
        drop(available);

        let mut stale = Vec::new();
        for generation in 0..24 {
            let pad = Arc::new(EventDevice::new_removable_gamepad(metadata.clone()).unwrap());
            assert_eq!(pad.name, name);
            assert!(EventDevice::new_removable_gamepad(metadata.clone()).is_err());
            let id = manager.register_device_with_name(name.clone(), pad.clone());
            let fd = manager.get_device_by_name(&name).unwrap().open().unwrap();
            pad.push_event(EV_KEY, 0x130, generation);
            let mut record = [0; InputEvent::size()];
            let reader = fd.as_char_device().unwrap();
            assert_eq!(reader.try_read_at(0, &mut record).unwrap(), record.len());
            assert_eq!(
                i32::from_ne_bytes(record[12..].try_into().unwrap()),
                generation
            );
            manager.unregister_device(id).unwrap();
            pad.detach_gamepad();
            assert!(manager.get_device_by_name(&name).is_none());
            assert!(fd.is_hung_up());
            assert!(matches!(
                reader.try_read_at(0, &mut record),
                Err(StreamError::EndOfStream)
            ));
            stale.push(fd);
        }
        let replacement = EventDevice::new_removable_gamepad(metadata.clone()).unwrap();
        assert_eq!(replacement.name, name);
        drop(stale);
        assert!(EventDevice::new_removable_gamepad(metadata).is_err());
    }

    #[test_case]
    fn gamepad_detach_notifies_all_subscribers_and_drains_final_snapshot_before_eof() {
        let metadata = InputDeviceMetadata::new(InputDeviceKind::Gamepad, INPUT_CAP_KEY);
        let pad = Arc::new(EventDevice::new_removable_gamepad(metadata).unwrap());
        let fd = pad.clone().open().unwrap();
        let neutral = [
            (EV_KEY, 0x130, 0),
            (EV_ABS, 0, 2048),
            (EV_SYN, SYN_REPORT, 0),
        ];
        for _ in 0..EVENT_QUEUE_CAPACITY - 1 {
            pad.push_event(EV_KEY, 0x130, 1);
        }
        pad.push_events(&neutral);
        let first = Arc::new(Waker::new_interruptible("gamepad-test-reader"));
        let second = Arc::new(Waker::new_interruptible("gamepad-test-reader"));
        let subscribe = |waker| {
            fd.subscribe_readiness(ReadyInterest::read(), waker)
                .unwrap()
                .pop()
                .unwrap()
        };
        let a = subscribe(&first);
        let b = subscribe(&second);
        pad.detach_gamepad();
        assert!(a.generation() > 0 && b.generation() > 0);
        assert!(pad.clone().open().is_err());
        pad.push_event(EV_KEY, 0x130, 1);

        let reader = fd.as_char_device().unwrap();
        let mut record = [0; InputEvent::size()];
        for (ty, code, value) in [(EV_SYN, SYN_DROPPED, 0), (EV_SYN, SYN_REPORT, 0)]
            .into_iter()
            .chain(neutral)
        {
            assert_eq!(reader.try_read_at(0, &mut record).unwrap(), record.len());
            assert_eq!(u16::from_ne_bytes(record[8..10].try_into().unwrap()), ty);
            assert_eq!(u16::from_ne_bytes(record[10..12].try_into().unwrap()), code);
            assert_eq!(i32::from_ne_bytes(record[12..].try_into().unwrap()), value);
        }
        for nonblocking in [false, true] {
            fd.set_nonblocking(nonblocking);
            assert!(fd.is_hung_up() && fd.current_ready(ReadyInterest::read()).read);
            assert_eq!(reader.read(&mut record), 0);
            assert!(matches!(
                reader.try_read_at(0, &mut record),
                Err(StreamError::EndOfStream)
            ));
        }
    }
}
