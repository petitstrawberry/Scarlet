//! Scarlet native sensor metadata and event-stream access.
//!
//! Sensor streams are independent from input `EV_ABS` streams. Applications
//! read one fixed-size [`SensorEvent`] at a time from `/dev/sensorN` and query
//! its physical metadata through [`SensorDevice::info`].

use crate::handle::capability::{StreamError, StreamResult};
use crate::handle::{Handle, HandleError, HandleResult};

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

/// Stable size of [`SensorInfo`] in ABI version 1.
pub const SENSOR_INFO_SIZE: usize = 80;
/// Stable size of [`SensorEvent`] in ABI version 1.
pub const SENSOR_EVENT_SIZE: usize = 40;

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

impl TryFrom<u32> for SensorType {
    type Error = ();

    fn try_from(value: u32) -> core::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Accelerometer),
            2 => Ok(Self::Gyroscope),
            3 => Ok(Self::Magnetometer),
            4 => Ok(Self::Proximity),
            5 => Ok(Self::Light),
            _ => Err(()),
        }
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

impl TryFrom<u32> for SensorLocation {
    type Error = ();

    fn try_from(value: u32) -> core::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Base),
            2 => Ok(Self::Lid),
            3 => Ok(Self::Camera),
            _ => Err(()),
        }
    }
}

/// Stable version-1 sensor metadata returned by [`SensorDevice::info`].
///
/// For vector sensors, `full_scale` is the positive sensor-native magnitude
/// represented by the full-scale raw code. Accelerometers use multiples of g,
/// gyroscopes use degrees per second, and magnetometers use microtesla. The
/// raw range and `resolution_bits` therefore provide enough information to
/// derive native-unit scale; conversion to SI units remains user-space policy.
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
    /// Padding reserved for future ABI versions; currently zero.
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
    /// Space reserved for forward-compatible ABI extensions; currently zero.
    pub reserved: [u32; 7],
}

/// One stable, fixed-size raw sensor event.
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

const _: [(); SENSOR_INFO_SIZE] = [(); core::mem::size_of::<SensorInfo>()];
const _: [(); SENSOR_EVENT_SIZE] = [(); core::mem::size_of::<SensorEvent>()];

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("fixed ABI field"),
    )
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_ne_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("fixed ABI field"),
    )
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("fixed ABI field"),
    )
}

fn parse_sensor_info(bytes: &[u8]) -> HandleResult<SensorInfo> {
    if bytes.len() != SENSOR_INFO_SIZE {
        return Err(HandleError::InvalidParameter);
    }
    let sensor_type =
        SensorType::try_from(read_u32(bytes, 8)).map_err(|_| HandleError::InvalidParameter)?;
    let location =
        SensorLocation::try_from(read_u32(bytes, 12)).map_err(|_| HandleError::InvalidParameter)?;
    let mut reserved = [0_u32; 7];
    for (index, word) in reserved.iter_mut().enumerate() {
        *word = read_u32(bytes, 52 + index * 4);
    }
    let info = SensorInfo {
        abi_version: read_u32(bytes, 0),
        struct_size: read_u32(bytes, 4),
        sensor_type,
        location,
        chip_id: read_u32(bytes, 16),
        axis_count: bytes[20],
        resolution_bits: bytes[21],
        reserved0: [bytes[22], bytes[23]],
        raw_min: read_i32(bytes, 24),
        raw_max: read_i32(bytes, 28),
        full_scale: read_u32(bytes, 32),
        min_frequency_millihz: read_u32(bytes, 36),
        max_frequency_millihz: read_u32(bytes, 40),
        current_frequency_millihz: read_u32(bytes, 44),
        fifo_capacity: read_u32(bytes, 48),
        reserved,
    };
    validate_sensor_info(&info)?;
    Ok(info)
}

fn validate_sensor_info(info: &SensorInfo) -> HandleResult<()> {
    let current_frequency_valid = info.current_frequency_millihz == 0
        || (info.min_frequency_millihz <= info.current_frequency_millihz
            && info.current_frequency_millihz <= info.max_frequency_millihz);
    if info.abi_version != SENSOR_ABI_VERSION
        || info.struct_size as usize != SENSOR_INFO_SIZE
        || info.sensor_type == SensorType::Unknown
        || info.axis_count == 0
        || info.axis_count > 3
        || info.raw_min >= info.raw_max
        || info.resolution_bits == 0
        || info.resolution_bits > 32
        || info.max_frequency_millihz == 0
        || info.min_frequency_millihz > info.max_frequency_millihz
        || !current_frequency_valid
        || info.reserved0 != [0; 2]
        || info.reserved != [0; 7]
    {
        return Err(HandleError::InvalidParameter);
    }
    if info.sensor_type.is_vector() {
        if info.axis_count != 3 || info.full_scale == 0 {
            return Err(HandleError::InvalidParameter);
        }
    } else if info.axis_count != 1 {
        return Err(HandleError::InvalidParameter);
    }
    Ok(())
}

fn parse_sensor_event(bytes: &[u8]) -> StreamResult<SensorEvent> {
    if bytes.len() != SENSOR_EVENT_SIZE {
        return Err(StreamError::InvalidParameter);
    }
    Ok(SensorEvent {
        timestamp_ns: read_u64(bytes, 0),
        sequence: read_u64(bytes, 8),
        values: [
            read_i32(bytes, 16),
            read_i32(bytes, 20),
            read_i32(bytes, 24),
        ],
        flags: read_u32(bytes, 28),
        lost_samples: read_u32(bytes, 32),
    })
}

/// Owning wrapper for a Scarlet native sensor event device.
#[derive(Debug)]
pub struct SensorDevice {
    handle: Handle,
}

impl SensorDevice {
    /// Open a sensor event stream for reading.
    ///
    /// # Arguments
    ///
    /// * `path` - Device path such as `/dev/sensor0`.
    ///
    /// # Returns
    ///
    /// A sensor wrapper, or a handle error if the path cannot be opened.
    pub fn open(path: &str) -> HandleResult<Self> {
        Self::from_handle(Handle::open(path, 0)?)
    }

    /// Wrap an owned stream-capable handle.
    ///
    /// # Arguments
    ///
    /// * `handle` - Handle whose ownership is transferred to this wrapper.
    ///
    /// # Returns
    ///
    /// A sensor wrapper, or [`HandleError::Unsupported`] if the object is not a stream.
    pub fn from_handle(handle: Handle) -> HandleResult<Self> {
        handle.as_stream()?;
        Ok(Self { handle })
    }

    /// Query and validate the device's stable sensor metadata.
    ///
    /// # Returns
    ///
    /// Validated version-1 metadata, or a handle error for a failed query or
    /// malformed kernel response.
    pub fn info(&self) -> HandleResult<SensorInfo> {
        let mut bytes = [0_u8; SENSOR_INFO_SIZE];
        self.handle
            .control(SCTL_SENSOR_GET_INFO, bytes.as_mut_ptr() as usize)?;
        parse_sensor_info(&bytes)
    }

    /// Read raw sensor stream bytes.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Destination byte buffer.
    ///
    /// # Returns
    ///
    /// Number of bytes read, or a stream error.
    pub fn read(&self, buffer: &mut [u8]) -> StreamResult<usize> {
        self.handle
            .as_stream()
            .map_err(|_| StreamError::Unsupported)?
            .read(buffer)
    }

    /// Read and decode one complete sensor event.
    ///
    /// # Returns
    ///
    /// `Ok(Some(event))` for one complete record, `Ok(None)` when a
    /// non-blocking read has no event, or an error for a malformed short read.
    pub fn read_event(&self) -> StreamResult<Option<SensorEvent>> {
        let mut bytes = [0_u8; SENSOR_EVENT_SIZE];
        let count = self.read(&mut bytes)?;
        if count == 0 {
            return Ok(None);
        }
        if count != SENSOR_EVENT_SIZE {
            return Err(StreamError::InvalidParameter);
        }
        parse_sensor_event(&bytes).map(Some)
    }

    /// Change whether reads block when no sample is available.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether reads should return without blocking.
    ///
    /// # Returns
    ///
    /// Success or a handle error from the kernel.
    pub fn set_nonblocking(&self, enabled: bool) -> HandleResult<()> {
        self.handle.set_nonblocking(enabled)
    }

    /// Borrow the underlying handle.
    ///
    /// # Returns
    ///
    /// The owned sensor handle.
    pub const fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Consume this wrapper and return its handle.
    ///
    /// # Returns
    ///
    /// The owned sensor handle.
    pub fn into_handle(self) -> Handle {
        self.handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_info_bytes() -> [u8; SENSOR_INFO_SIZE] {
        let mut bytes = [0_u8; SENSOR_INFO_SIZE];
        bytes[0..4].copy_from_slice(&1_u32.to_ne_bytes());
        bytes[4..8].copy_from_slice(&(SENSOR_INFO_SIZE as u32).to_ne_bytes());
        bytes[8..12].copy_from_slice(&(SensorType::Accelerometer as u32).to_ne_bytes());
        bytes[12..16].copy_from_slice(&(SensorLocation::Lid as u32).to_ne_bytes());
        bytes[16..20].copy_from_slice(&9_u32.to_ne_bytes());
        bytes[20] = 3;
        bytes[21] = 16;
        bytes[24..28].copy_from_slice(&(-32768_i32).to_ne_bytes());
        bytes[28..32].copy_from_slice(&32767_i32.to_ne_bytes());
        bytes[32..36].copy_from_slice(&4_u32.to_ne_bytes());
        bytes[36..40].copy_from_slice(&12_500_u32.to_ne_bytes());
        bytes[40..44].copy_from_slice(&100_000_u32.to_ne_bytes());
        bytes[44..48].copy_from_slice(&50_000_u32.to_ne_bytes());
        bytes[48..52].copy_from_slice(&32_u32.to_ne_bytes());
        bytes
    }

    #[test]
    fn sensor_abi_layout_and_enum_values_are_stable() {
        assert_eq!(core::mem::size_of::<SensorInfo>(), SENSOR_INFO_SIZE);
        assert_eq!(core::mem::size_of::<SensorEvent>(), SENSOR_EVENT_SIZE);
        assert_eq!(SensorType::Accelerometer as u32, 1);
        assert_eq!(SensorType::Light as u32, 5);
        assert_eq!(SensorLocation::Base as u32, 1);
        assert_eq!(SensorLocation::Camera as u32, 3);
        assert_eq!(SCTL_SENSOR_GET_INFO, 0x5353_0200);
    }

    #[test]
    fn sensor_info_parser_validates_malformed_enums_and_headers() {
        let bytes = valid_info_bytes();
        let info = parse_sensor_info(&bytes).unwrap();
        assert_eq!(info.sensor_type, SensorType::Accelerometer);
        assert_eq!(info.location, SensorLocation::Lid);
        assert_eq!(info.full_scale, 4);

        let mut suspended = bytes;
        suspended[44..48].copy_from_slice(&0_u32.to_ne_bytes());
        assert_eq!(
            parse_sensor_info(&suspended)
                .unwrap()
                .current_frequency_millihz,
            0
        );

        let mut invalid = bytes;
        invalid[8..12].copy_from_slice(&99_u32.to_ne_bytes());
        assert_eq!(
            parse_sensor_info(&invalid),
            Err(HandleError::InvalidParameter)
        );
        assert_eq!(
            parse_sensor_info(&bytes[..79]),
            Err(HandleError::InvalidParameter)
        );
    }

    #[test]
    fn sensor_event_parser_requires_exact_size() {
        let mut bytes = [0_u8; SENSOR_EVENT_SIZE];
        bytes[0..8].copy_from_slice(&123_u64.to_ne_bytes());
        bytes[8..16].copy_from_slice(&7_u64.to_ne_bytes());
        bytes[16..20].copy_from_slice(&(-1_i32).to_ne_bytes());
        bytes[28..32].copy_from_slice(&SENSOR_EVENT_FLAG_SAMPLE.to_ne_bytes());
        let event = parse_sensor_event(&bytes).unwrap();
        assert_eq!((event.timestamp_ns, event.sequence), (123, 7));
        assert_eq!(event.values[0], -1);
        assert_eq!(
            parse_sensor_event(&bytes[..39]),
            Err(StreamError::InvalidParameter)
        );
    }
}
