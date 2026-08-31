//! Generic sensor metadata and event-stream devices.
//!
//! Sensor samples use a dedicated `/dev/sensorN` ABI. They deliberately do
//! not share the input subsystem's `EV_ABS` namespace, so motion and ambient
//! sensors can never be mistaken for pointer devices.

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::arch::Trapframe;
use crate::device::char::CharDevice;
use crate::device::{Device, DeviceCapability, DeviceType};
use crate::library::std::usercopy::copy_to_user;
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::object::capability::{ControlOps, MemoryMappingInfo, MemoryMappingOps};
use crate::sync::{IrqSpinLock, Waker};

/// Version implemented by the stable Scarlet sensor ABI.
pub const SENSOR_ABI_VERSION: u32 = 1;
/// Copy the device's [`SensorInfo`] to the user pointer passed as the argument.
pub const SCTL_SENSOR_GET_INFO: u32 = 0x5353_0200;

/// The event contains one sample in [`SensorEvent::values`].
pub const SENSOR_EVENT_FLAG_SAMPLE: u32 = 1 << 0;
/// The event marks completion of a requested FIFO flush.
pub const SENSOR_EVENT_FLAG_FLUSH: u32 = 1 << 1;
/// The sample was capable of waking the system.
pub const SENSOR_EVENT_FLAG_WAKEUP: u32 = 1 << 2;
/// The timestamp was reconstructed or otherwise approximate.
pub const SENSOR_EVENT_FLAG_TIMESTAMP_APPROXIMATE: u32 = 1 << 3;
/// Samples were lost before this event.
pub const SENSOR_EVENT_FLAG_DATA_LOST: u32 = 1 << 4;

const SENSOR_EVENT_FLAGS: u32 = SENSOR_EVENT_FLAG_SAMPLE
    | SENSOR_EVENT_FLAG_FLUSH
    | SENSOR_EVENT_FLAG_WAKEUP
    | SENSOR_EVENT_FLAG_TIMESTAMP_APPROXIMATE
    | SENSOR_EVENT_FLAG_DATA_LOST;

const SENSOR_QUEUE_CAPACITY: usize = 256;
const SCTL_SET_NONBLOCKING: u32 = 0x5353_0007;
static SENSOR_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Physical quantity represented by a sensor stream.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SensorType {
    /// The sensor type is unspecified.
    #[default]
    Unknown = 0,
    /// Three-axis linear acceleration.
    Accelerometer = 1,
    /// Three-axis angular velocity.
    Gyroscope = 2,
    /// Three-axis magnetic field strength.
    Magnetometer = 3,
    /// Scalar proximity measurement.
    Proximity = 4,
    /// Scalar ambient-light measurement.
    Light = 5,
}

impl SensorType {
    fn is_vector(self) -> bool {
        matches!(
            self,
            Self::Accelerometer | Self::Gyroscope | Self::Magnetometer
        )
    }
}

/// Physical placement of a sensor in the system.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SensorLocation {
    /// The placement is unspecified.
    #[default]
    Unknown = 0,
    /// The sensor is in the keyboard/base half of a convertible.
    Base = 1,
    /// The sensor is in the display/lid half of a convertible.
    Lid = 2,
    /// The sensor is associated with a camera module.
    Camera = 3,
}

/// Stable version-1 sensor metadata copied by [`SCTL_SENSOR_GET_INFO`].
///
/// `raw_min`, `raw_max`, `resolution_bits`, and `full_scale` describe the
/// conversion from raw samples. For vector sensors, `full_scale` is the
/// positive sensor-native magnitude represented by the full-scale code:
/// accelerometers use multiples of g, gyroscopes use degrees per second, and
/// magnetometers use microtesla. User space derives the native-unit scale from
/// the raw full-scale code and performs any desired SI conversion. Scalar
/// sensors may use `full_scale == 0` when their driver defines raw units.
///
/// The reserved words are zero in ABI version 1 and must be ignored by readers.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SensorInfo {
    /// ABI version; currently [`SENSOR_ABI_VERSION`].
    pub abi_version: u32,
    /// Size of this structure in bytes.
    pub struct_size: u32,
    /// Physical quantity emitted by the device.
    pub sensor_type: SensorType,
    /// Physical placement of the sensor.
    pub location: SensorLocation,
    /// Driver-defined stable chip or EC sensor identifier.
    pub chip_id: u32,
    /// Number of meaningful entries in each event's values array.
    pub axis_count: u8,
    /// Effective signed raw sample resolution.
    pub resolution_bits: u8,
    /// Padding reserved for future ABI versions; always zero.
    pub reserved0: [u8; 2],
    /// Inclusive minimum raw sample value.
    pub raw_min: i32,
    /// Inclusive maximum raw sample value.
    pub raw_max: i32,
    /// Positive full-scale magnitude in sensor-native units.
    pub full_scale: u32,
    /// Minimum supported sampling frequency in millihertz.
    pub min_frequency_millihz: u32,
    /// Maximum supported sampling frequency in millihertz.
    pub max_frequency_millihz: u32,
    /// Sampling frequency currently configured, in millihertz.
    /// A value of zero means the sensor is currently suspended.
    pub current_frequency_millihz: u32,
    /// Number of samples the hardware FIFO can retain, or zero if absent.
    pub fifo_capacity: u32,
    /// Space reserved for forward-compatible ABI extensions; always zero.
    pub reserved: [u32; 7],
}

impl SensorInfo {
    /// Construct and validate stable version-1 sensor metadata.
    ///
    /// # Arguments
    ///
    /// * `sensor_type` - Physical quantity represented by the stream.
    /// * `location` - Physical placement of the device.
    /// * `chip_id` - Driver-defined stable chip or EC sensor identifier.
    /// * `axis_count` - Number of meaningful sample axes, from one to three.
    /// * `raw_min` - Inclusive minimum raw code.
    /// * `raw_max` - Inclusive maximum raw code.
    /// * `full_scale` - Positive native-unit full-scale magnitude for vector sensors.
    /// * `resolution_bits` - Effective signed resolution, from one to 32 bits.
    /// * `min_frequency_millihz` - Minimum sampling frequency in millihertz.
    /// * `max_frequency_millihz` - Maximum sampling frequency in millihertz.
    /// * `current_frequency_millihz` - Current frequency, or zero when suspended.
    /// * `fifo_capacity` - Hardware FIFO capacity in samples, or zero.
    ///
    /// # Returns
    ///
    /// Initialized metadata, or an error if the description is inconsistent.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sensor_type: SensorType,
        location: SensorLocation,
        chip_id: u32,
        axis_count: u8,
        raw_min: i32,
        raw_max: i32,
        full_scale: u32,
        resolution_bits: u8,
        min_frequency_millihz: u32,
        max_frequency_millihz: u32,
        current_frequency_millihz: u32,
        fifo_capacity: u32,
    ) -> Result<Self, &'static str> {
        let info = Self {
            abi_version: SENSOR_ABI_VERSION,
            struct_size: size_of::<Self>() as u32,
            sensor_type,
            location,
            chip_id,
            axis_count,
            resolution_bits,
            reserved0: [0; 2],
            raw_min,
            raw_max,
            full_scale,
            min_frequency_millihz,
            max_frequency_millihz,
            current_frequency_millihz,
            fifo_capacity,
            reserved: [0; 7],
        };
        info.validate()?;
        Ok(info)
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.abi_version != SENSOR_ABI_VERSION || self.struct_size as usize != size_of::<Self>()
        {
            return Err("Invalid sensor metadata ABI header");
        }
        if self.sensor_type == SensorType::Unknown {
            return Err("Sensor type must be declared");
        }
        if self.axis_count == 0 || self.axis_count > 3 {
            return Err("Sensor axis count is out of range");
        }
        if self.sensor_type.is_vector() && self.axis_count != 3 {
            return Err("Vector sensors must declare three axes");
        }
        if !self.sensor_type.is_vector() && self.axis_count != 1 {
            return Err("Scalar sensors must declare one axis");
        }
        if self.raw_min >= self.raw_max {
            return Err("Sensor raw range is invalid");
        }
        if self.resolution_bits == 0 || self.resolution_bits > 32 {
            return Err("Sensor resolution is out of range");
        }
        if self.sensor_type.is_vector() && self.full_scale == 0 {
            return Err("Vector sensor full scale must be nonzero");
        }
        let current_frequency_valid = self.current_frequency_millihz == 0
            || (self.min_frequency_millihz <= self.current_frequency_millihz
                && self.current_frequency_millihz <= self.max_frequency_millihz);
        if self.max_frequency_millihz == 0
            || self.min_frequency_millihz > self.max_frequency_millihz
            || !current_frequency_valid
        {
            return Err("Sensor sampling frequency is invalid");
        }
        if self.reserved0 != [0; 2] || self.reserved != [0; 7] {
            return Err("Sensor metadata reserved fields must be zero");
        }
        Ok(())
    }
}

/// One stable, fixed-size sensor event record.
///
/// Values are signed raw sensor codes. Their physical scale is derived from
/// the associated [`SensorInfo`] raw range, resolution, and full-scale fields.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SensorEvent {
    /// Monotonic sample timestamp in nanoseconds.
    pub timestamp_ns: u64,
    /// Per-device push-attempt sequence number.
    pub sequence: u64,
    /// Raw axis values; only `SensorInfo::axis_count` entries are meaningful.
    pub values: [i32; 3],
    /// `SENSOR_EVENT_FLAG_*` bit mask.
    pub flags: u32,
    /// Number of source or queue-overflow samples lost before this event.
    pub lost_samples: u32,
}

const _: [(); 80] = [(); size_of::<SensorInfo>()];
const _: [(); 40] = [(); size_of::<SensorEvent>()];

/// Read-only character device exposing a sensor sample stream.
pub struct SensorDevice {
    name: String,
    info: SensorInfo,
    queue: IrqSpinLock<VecDeque<SensorEvent>>,
    next_sequence: AtomicU64,
    waker: Waker,
    nonblocking: IrqSpinLock<bool>,
}

impl SensorDevice {
    /// Create a validated `/dev/sensorN` event device.
    ///
    /// # Arguments
    ///
    /// * `info` - Stable sensor metadata returned to user space.
    ///
    /// # Returns
    ///
    /// A new sensor device, or an error if the metadata is inconsistent.
    pub fn new(info: SensorInfo) -> Result<Self, &'static str> {
        info.validate()?;
        let index = SENSOR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let name = alloc::format!("sensor{index}");
        let waker_name = alloc::format!("sensor_event_{index}").leak();
        Ok(Self {
            name,
            info,
            queue: IrqSpinLock::new(VecDeque::with_capacity(SENSOR_QUEUE_CAPACITY)),
            next_sequence: AtomicU64::new(1),
            waker: Waker::new_interruptible(waker_name),
            nonblocking: IrqSpinLock::new(false),
        })
    }

    /// Return the assigned device node name.
    ///
    /// # Returns
    ///
    /// A name such as `sensor0`.
    pub fn get_name(&self) -> &str {
        &self.name
    }

    /// Queue one raw sample with an explicit monotonic timestamp.
    ///
    /// Every call consumes a sequence number, including a call which forces an
    /// old queued event to be discarded. On overflow the oldest event is
    /// removed and the next unread event reports that event's accumulated loss
    /// plus one, preserving the order of all surviving samples.
    ///
    /// # Arguments
    ///
    /// * `timestamp_ns` - Monotonic sample time in nanoseconds.
    /// * `values` - Up to three signed raw axis values.
    /// * `flags` - Additional `SENSOR_EVENT_FLAG_*` attributes.
    /// * `source_lost` - Samples reported lost by the hardware before this one.
    ///
    /// # Returns
    ///
    /// Success after enqueueing the sample, or an error for unknown flag bits.
    pub fn push_sample_at(
        &self,
        timestamp_ns: u64,
        values: [i32; 3],
        flags: u32,
        source_lost: u32,
    ) -> Result<(), &'static str> {
        self.push_event_at(
            timestamp_ns,
            values,
            flags | SENSOR_EVENT_FLAG_SAMPLE,
            source_lost,
        )
    }

    /// Queue one sample or FIFO-flush event with an explicit timestamp.
    ///
    /// Unlike [`SensorDevice::push_sample_at`], this method permits a
    /// `SENSOR_EVENT_FLAG_FLUSH` event without `SENSOR_EVENT_FLAG_SAMPLE`.
    /// At least one of those two record-kind flags must be present, and unknown
    /// flag bits are rejected.
    ///
    /// # Arguments
    ///
    /// * `timestamp_ns` - Monotonic event time in nanoseconds.
    /// * `values` - Raw sample axes, or zeroes for a flush-only event.
    /// * `flags` - Complete `SENSOR_EVENT_FLAG_*` bit mask.
    /// * `source_lost` - Samples reported lost before this event.
    ///
    /// # Returns
    ///
    /// Success after enqueueing the event, or an error for invalid flags.
    pub fn push_event_at(
        &self,
        timestamp_ns: u64,
        values: [i32; 3],
        flags: u32,
        source_lost: u32,
    ) -> Result<(), &'static str> {
        if flags & !SENSOR_EVENT_FLAGS != 0 {
            return Err("Sensor event contains unknown flags");
        }
        if flags & (SENSOR_EVENT_FLAG_SAMPLE | SENSOR_EVENT_FLAG_FLUSH) == 0 {
            return Err("Sensor event has no record kind");
        }
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let mut event = SensorEvent {
            timestamp_ns,
            sequence,
            values,
            flags,
            lost_samples: source_lost,
        };
        if event.lost_samples != 0 {
            event.flags |= SENSOR_EVENT_FLAG_DATA_LOST;
        }

        {
            let mut queue = self.queue.lock();
            if queue.len() >= SENSOR_QUEUE_CAPACITY {
                if let Some(dropped) = queue.pop_front() {
                    if let Some(next) = queue.front_mut() {
                        next.lost_samples = next
                            .lost_samples
                            .saturating_add(dropped.lost_samples)
                            .saturating_add(1);
                        next.flags |= SENSOR_EVENT_FLAG_DATA_LOST;
                    }
                }
            }
            queue.push_back(event);
        }
        self.waker.wake_one();
        Ok(())
    }

    fn has_events(&self) -> bool {
        !self.queue.lock().is_empty()
    }

    fn pop_event(&self, buffer: &mut [u8]) -> usize {
        if buffer.len() < size_of::<SensorEvent>() {
            return 0;
        }
        let Some(event) = self.queue.lock().pop_front() else {
            return 0;
        };
        // Serialize fields explicitly so the four bytes of repr(C) tail
        // padding are deterministically zero and never expose kernel stack data.
        buffer[..size_of::<SensorEvent>()].fill(0);
        buffer[0..8].copy_from_slice(&event.timestamp_ns.to_ne_bytes());
        buffer[8..16].copy_from_slice(&event.sequence.to_ne_bytes());
        buffer[16..20].copy_from_slice(&event.values[0].to_ne_bytes());
        buffer[20..24].copy_from_slice(&event.values[1].to_ne_bytes());
        buffer[24..28].copy_from_slice(&event.values[2].to_ne_bytes());
        buffer[28..32].copy_from_slice(&event.flags.to_ne_bytes());
        buffer[32..36].copy_from_slice(&event.lost_samples.to_ne_bytes());
        size_of::<SensorEvent>()
    }
}

impl Device for SensorDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "sensor_device"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn capabilities(&self) -> &'static [DeviceCapability] {
        &[DeviceCapability::Sensor]
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for SensorDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        let bytes = self.pop_event(buffer);
        if bytes != 0 || buffer.len() < size_of::<SensorEvent>() || *self.nonblocking.lock() {
            return bytes;
        }
        if let Some(task) = crate::task::mytask() {
            self.waker.wait(task.get_id(), task.get_trapframe());
            return self.pop_event(buffer);
        }
        0
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        Ok(self.read(buffer))
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("Write not supported on sensor devices")
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("Write not supported on sensor devices")
    }

    fn can_read(&self) -> bool {
        self.has_events()
    }

    fn can_write(&self) -> bool {
        false
    }
}

impl Selectable for SensorDevice {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut ready = ReadySet::none();
        ready.read = interest.read && self.has_events();
        ready
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        _trapframe: &mut Trapframe,
        timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        if !interest.read || self.has_events() || *self.nonblocking.lock() {
            SelectWaitOutcome::Ready
        } else if timeout_ticks.is_some() {
            SelectWaitOutcome::TimedOut
        } else {
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

impl ControlOps for SensorDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            SCTL_SET_NONBLOCKING => {
                self.set_nonblocking(arg != 0);
                Ok(0)
            }
            SCTL_SENSOR_GET_INFO => {
                let task =
                    crate::task::mytask().ok_or("No current task for sensor metadata copy")?;
                // SAFETY: SensorInfo is fully initialized, repr(C), and its
                // compile-time layout assertion fixes the copied byte count.
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        (&self.info as *const SensorInfo).cast::<u8>(),
                        size_of::<SensorInfo>(),
                    )
                };
                copy_to_user(&task, arg, bytes)
                    .map_err(|_| "Failed to copy sensor metadata to user")?;
                Ok(0)
            }
            _ => Err("Control operation not supported on sensor device"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![(SCTL_SENSOR_GET_INFO, "Get sensor metadata")]
    }
}

impl MemoryMappingOps for SensorDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported on sensor devices")
    }

    fn supports_mmap(&self) -> bool {
        false
    }

    fn supports_private_mmap(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accelerometer_info() -> SensorInfo {
        SensorInfo::new(
            SensorType::Accelerometer,
            SensorLocation::Base,
            7,
            3,
            -32768,
            32767,
            4,
            16,
            12_500,
            100_000,
            50_000,
            32,
        )
        .unwrap()
    }

    fn decode(bytes: &[u8]) -> SensorEvent {
        assert_eq!(bytes.len(), size_of::<SensorEvent>());
        // SAFETY: the byte slice has exactly one SensorEvent and read_unaligned
        // does not require its starting address to have SensorEvent alignment.
        unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<SensorEvent>()) }
    }

    #[test_case]
    fn sensor_metadata_and_layout_are_validated() {
        assert_eq!(size_of::<SensorInfo>(), 80);
        assert_eq!(size_of::<SensorEvent>(), 40);
        assert_eq!(SensorType::Accelerometer as u32, 1);
        assert_eq!(SensorType::Light as u32, 5);
        assert_eq!(SensorLocation::Base as u32, 1);
        assert_eq!(SensorLocation::Camera as u32, 3);
        assert_eq!(SCTL_SENSOR_GET_INFO, 0x5353_0200);
        let info = accelerometer_info();
        assert_eq!(info.abi_version, 1);
        assert_eq!(info.struct_size, 80);
        assert_eq!(info.full_scale, 4);
        assert!(
            SensorInfo::new(
                SensorType::Gyroscope,
                SensorLocation::Lid,
                8,
                3,
                -32768,
                32767,
                2000,
                16,
                12_500,
                100_000,
                0,
                32,
            )
            .is_ok()
        );
        assert!(
            SensorInfo::new(
                SensorType::Unknown,
                SensorLocation::Unknown,
                0,
                1,
                0,
                1,
                0,
                8,
                0,
                1,
                1,
                0,
            )
            .is_err()
        );
        assert!(
            SensorInfo::new(
                SensorType::Accelerometer,
                SensorLocation::Base,
                0,
                3,
                -1,
                1,
                0,
                16,
                1,
                1,
                1,
                0,
            )
            .is_err()
        );
    }

    #[test_case]
    fn sensor_samples_remain_ordered() {
        let dev = SensorDevice::new(accelerometer_info()).unwrap();
        dev.push_sample_at(10, [1, 2, 3], 0, 0).unwrap();
        dev.push_sample_at(20, [4, 5, 6], SENSOR_EVENT_FLAG_WAKEUP, 0)
            .unwrap();
        let mut bytes = [0_u8; 40];
        assert_eq!(dev.read(&mut bytes), 40);
        let first = decode(&bytes);
        assert_eq!((first.timestamp_ns, first.sequence), (10, 1));
        assert_eq!(first.values, [1, 2, 3]);
        assert_eq!(dev.read(&mut bytes), 40);
        let second = decode(&bytes);
        assert_eq!((second.timestamp_ns, second.sequence), (20, 2));
        assert_ne!(second.flags & SENSOR_EVENT_FLAG_WAKEUP, 0);
    }

    #[test_case]
    fn sensor_overflow_marks_next_unread_event_and_sequence_gap() {
        let dev = SensorDevice::new(accelerometer_info()).unwrap();
        dev.push_sample_at(0, [0; 3], 0, 4).unwrap();
        for i in 1..=SENSOR_QUEUE_CAPACITY {
            dev.push_sample_at(i as u64, [i as i32, 0, 0], 0, 0)
                .unwrap();
        }
        assert_eq!(dev.queue.lock().len(), SENSOR_QUEUE_CAPACITY);
        let mut bytes = [0_u8; 40];
        assert_eq!(dev.read(&mut bytes), 40);
        let event = decode(&bytes);
        assert_eq!(event.sequence, 2);
        assert_eq!(event.values[0], 1);
        assert_eq!(event.lost_samples, 5);
        assert_ne!(event.flags & SENSOR_EVENT_FLAG_DATA_LOST, 0);
    }

    #[test_case]
    fn sensor_nonblocking_and_short_reads_return_zero() {
        let dev = SensorDevice::new(accelerometer_info()).unwrap();
        dev.set_nonblocking(true);
        let mut bytes = [0_u8; 40];
        assert_eq!(dev.read(&mut bytes), 0);
        dev.push_sample_at(1, [1, 2, 3], 0, 0).unwrap();
        assert_eq!(dev.read(&mut bytes[..39]), 0);
        assert!(dev.has_events());
        assert_eq!(dev.read(&mut bytes), 40);
    }

    #[test_case]
    fn sensor_flush_only_events_and_flags_are_validated() {
        let dev = SensorDevice::new(accelerometer_info()).unwrap();
        assert!(
            dev.push_event_at(1, [0; 3], SENSOR_EVENT_FLAG_FLUSH, 0)
                .is_ok()
        );
        assert!(dev.push_event_at(2, [0; 3], 0, 0).is_err());
        assert!(
            dev.push_event_at(3, [0; 3], SENSOR_EVENT_FLAG_SAMPLE | (1 << 31), 0)
                .is_err()
        );
        let mut bytes = [0_u8; 40];
        assert_eq!(dev.read(&mut bytes), 40);
        let event = decode(&bytes);
        assert_eq!(event.flags, SENSOR_EVENT_FLAG_FLUSH);
    }
}
