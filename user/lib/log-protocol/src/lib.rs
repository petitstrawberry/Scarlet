//! Wire protocol shared by Scarlet's logging daemon and its clients.
//!
//! Every message starts with an eight-byte little-endian [`Header`]. The
//! payload that follows is selected by `message_type`. The protocol keeps log
//! text as bytes so stdout and stderr remain observable even when a process
//! emits invalid UTF-8.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Local socket used by `logd`.
pub const SOCKET_PATH: &str = "/tmp/logd.sock";

/// Framed message header size in bytes.
pub const HEADER_SIZE: usize = 8;

/// Largest unit name accepted by the protocol.
pub const MAX_UNIT_LEN: usize = 256;

/// Largest single log record accepted by the protocol.
pub const MAX_MESSAGE_LEN: usize = 48 * 1024;

/// Largest framed payload accepted by either endpoint.
pub const MAX_PAYLOAD_SIZE: usize = 64 * 1024;

/// Append one stdout, stderr, or structured record.
pub const MSG_APPEND: u32 = 0x0001;

/// Query stored records and optionally keep following new records.
pub const MSG_QUERY: u32 = 0x0002;

/// One record returned by `logd`.
pub const MSG_RECORD: u32 = 0x1000;

/// End of a finite query response.
pub const MSG_QUERY_END: u32 = 0x1001;

/// Protocol or query error returned by `logd`.
pub const MSG_ERROR: u32 = 0x10ff;

/// No PID filter in a [`Query`].
pub const ANY_PID: i32 = -1;

/// No priority filter in a [`Query`].
pub const ANY_PRIORITY: u8 = u8::MAX;

/// Error returned while encoding or decoding a log protocol payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// A string, byte field, or payload exceeded its protocol limit.
    InvalidLength,
    /// An enum discriminant or reserved value was invalid.
    InvalidValue,
    /// The payload ended before all declared fields were available.
    Truncated,
    /// Bytes remained after a complete payload was decoded.
    TrailingBytes,
}

/// Origin stream for a log record.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogStream {
    /// Process standard output.
    Stdout = 1,
    /// Process standard error.
    Stderr = 2,
    /// A record submitted directly by a system component.
    Internal = 3,
}

impl LogStream {
    /// Decode a stream discriminant.
    ///
    /// # Arguments
    ///
    /// * `value` - Wire-format stream value.
    ///
    /// # Returns
    ///
    /// The corresponding stream, or `None` for an unknown value.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Stdout),
            2 => Some(Self::Stderr),
            3 => Some(Self::Internal),
            _ => None,
        }
    }

    /// Return the stable wire-format stream value.
    ///
    /// # Returns
    ///
    /// The numeric stream discriminant.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Return a human-readable stream name.
    ///
    /// # Returns
    ///
    /// `stdout`, `stderr`, or `internal`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Internal => "internal",
        }
    }
}

/// Syslog-compatible log priority.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LogPriority {
    /// System is unusable.
    Emergency = 0,
    /// Immediate action is required.
    Alert = 1,
    /// Critical failure.
    Critical = 2,
    /// Error condition.
    Error = 3,
    /// Warning condition.
    Warning = 4,
    /// Significant but expected condition.
    Notice = 5,
    /// Informational message.
    Info = 6,
    /// Debug-only message.
    Debug = 7,
}

impl LogPriority {
    /// Decode a syslog priority value.
    ///
    /// # Arguments
    ///
    /// * `value` - Wire-format priority in the inclusive range 0 through 7.
    ///
    /// # Returns
    ///
    /// The corresponding priority, or `None` when out of range.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Emergency),
            1 => Some(Self::Alert),
            2 => Some(Self::Critical),
            3 => Some(Self::Error),
            4 => Some(Self::Warning),
            5 => Some(Self::Notice),
            6 => Some(Self::Info),
            7 => Some(Self::Debug),
            _ => None,
        }
    }

    /// Parse a textual or numeric priority.
    ///
    /// # Arguments
    ///
    /// * `value` - Priority such as `warning`, `warn`, `4`, or `debug`.
    ///
    /// # Returns
    ///
    /// The parsed priority, or `None` for an unsupported value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "0" | "emerg" | "emergency" => Some(Self::Emergency),
            "1" | "alert" => Some(Self::Alert),
            "2" | "crit" | "critical" => Some(Self::Critical),
            "3" | "err" | "error" => Some(Self::Error),
            "4" | "warn" | "warning" => Some(Self::Warning),
            "5" | "notice" => Some(Self::Notice),
            "6" | "info" => Some(Self::Info),
            "7" | "debug" => Some(Self::Debug),
            _ => None,
        }
    }

    /// Return the stable wire-format priority value.
    ///
    /// # Returns
    ///
    /// The syslog-compatible priority number.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Return the canonical lowercase priority name.
    ///
    /// # Returns
    ///
    /// A stable human-readable priority name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Emergency => "emerg",
            Self::Alert => "alert",
            Self::Critical => "crit",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Notice => "notice",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

/// Header preceding every framed protocol message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    /// Message type constant.
    pub message_type: u32,
    /// Number of payload bytes following this header.
    pub payload_size: u32,
}

impl Header {
    /// Serialize this header as little-endian bytes.
    ///
    /// # Returns
    ///
    /// The fixed-width wire representation.
    pub fn to_le_bytes(self) -> [u8; HEADER_SIZE] {
        let mut bytes = [0u8; HEADER_SIZE];
        bytes[0..4].copy_from_slice(&self.message_type.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.payload_size.to_le_bytes());
        bytes
    }

    /// Decode a little-endian header.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Exactly [`HEADER_SIZE`] bytes.
    ///
    /// # Returns
    ///
    /// The decoded header.
    pub fn from_le_bytes(bytes: [u8; HEADER_SIZE]) -> Self {
        Self {
            message_type: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            payload_size: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }
}

/// Record submitted to `logd` by a service manager or structured client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppendRequest {
    /// Service or application identity.
    pub unit: String,
    /// Originating process ID.
    pub pid: i32,
    /// Origin stream.
    pub stream: LogStream,
    /// Record priority.
    pub priority: LogPriority,
    /// Uninterpreted log message bytes without the line delimiter.
    pub message: Vec<u8>,
}

impl AppendRequest {
    /// Encode this append request.
    ///
    /// # Returns
    ///
    /// The payload bytes, or an error when a field exceeds protocol limits.
    pub fn to_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        validate_text_fields(self.unit.as_bytes(), &self.message)?;
        let mut out = Vec::with_capacity(16 + self.unit.len() + self.message.len());
        push_u32(&mut out, self.unit.len() as u32);
        push_i32(&mut out, self.pid);
        out.push(self.stream.as_u8());
        out.push(self.priority.as_u8());
        out.extend_from_slice(&0u16.to_le_bytes());
        push_u32(&mut out, self.message.len() as u32);
        out.extend_from_slice(self.unit.as_bytes());
        out.extend_from_slice(&self.message);
        validate_payload_size(&out)?;
        Ok(out)
    }

    /// Decode an append request payload.
    ///
    /// # Arguments
    ///
    /// * `payload` - Bytes following a [`MSG_APPEND`] header.
    ///
    /// # Returns
    ///
    /// The decoded request, or a protocol error.
    pub fn from_payload(payload: &[u8]) -> Result<Self, ProtocolError> {
        validate_payload_size(payload)?;
        let mut cursor = Cursor::new(payload);
        let unit_len = cursor.u32()? as usize;
        let pid = cursor.i32()?;
        let stream = LogStream::from_u8(cursor.u8()?).ok_or(ProtocolError::InvalidValue)?;
        let priority = LogPriority::from_u8(cursor.u8()?).ok_or(ProtocolError::InvalidValue)?;
        if cursor.u16()? != 0 {
            return Err(ProtocolError::InvalidValue);
        }
        let message_len = cursor.u32()? as usize;
        if unit_len > MAX_UNIT_LEN || message_len > MAX_MESSAGE_LEN {
            return Err(ProtocolError::InvalidLength);
        }
        let unit = decode_unit(cursor.bytes(unit_len)?)?;
        let message = cursor.bytes(message_len)?.to_vec();
        cursor.finish()?;
        Ok(Self {
            unit,
            pid,
            stream,
            priority,
            message,
        })
    }
}

/// Query for historical records and an optional live stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Query {
    /// Return only records with a greater sequence number.
    pub after_sequence: u64,
    /// Limit the initial response to the newest number of matching records.
    pub tail: u32,
    /// Keep the connection open and stream matching records after the snapshot.
    pub follow: bool,
    /// Exact unit filter; an empty string matches every unit.
    pub unit: String,
    /// Exact PID filter, or [`ANY_PID`].
    pub pid: i32,
    /// Most verbose accepted priority, or [`ANY_PRIORITY`].
    pub max_priority: u8,
}

impl Query {
    /// Encode this query.
    ///
    /// # Returns
    ///
    /// The payload bytes, or an error when a field is invalid.
    pub fn to_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.unit.len() > MAX_UNIT_LEN {
            return Err(ProtocolError::InvalidLength);
        }
        if self.max_priority != ANY_PRIORITY && LogPriority::from_u8(self.max_priority).is_none() {
            return Err(ProtocolError::InvalidValue);
        }
        let mut out = Vec::with_capacity(24 + self.unit.len());
        push_u64(&mut out, self.after_sequence);
        push_u32(&mut out, self.tail);
        out.push(u8::from(self.follow));
        out.push(self.max_priority);
        out.extend_from_slice(&0u16.to_le_bytes());
        push_i32(&mut out, self.pid);
        push_u32(&mut out, self.unit.len() as u32);
        out.extend_from_slice(self.unit.as_bytes());
        validate_payload_size(&out)?;
        Ok(out)
    }

    /// Decode a query payload.
    ///
    /// # Arguments
    ///
    /// * `payload` - Bytes following a [`MSG_QUERY`] header.
    ///
    /// # Returns
    ///
    /// The decoded query, or a protocol error.
    pub fn from_payload(payload: &[u8]) -> Result<Self, ProtocolError> {
        validate_payload_size(payload)?;
        let mut cursor = Cursor::new(payload);
        let after_sequence = cursor.u64()?;
        let tail = cursor.u32()?;
        let follow = match cursor.u8()? {
            0 => false,
            1 => true,
            _ => return Err(ProtocolError::InvalidValue),
        };
        let max_priority = cursor.u8()?;
        if max_priority != ANY_PRIORITY && LogPriority::from_u8(max_priority).is_none() {
            return Err(ProtocolError::InvalidValue);
        }
        if cursor.u16()? != 0 {
            return Err(ProtocolError::InvalidValue);
        }
        let pid = cursor.i32()?;
        let unit_len = cursor.u32()? as usize;
        if unit_len > MAX_UNIT_LEN {
            return Err(ProtocolError::InvalidLength);
        }
        let unit = decode_unit(cursor.bytes(unit_len)?)?;
        cursor.finish()?;
        Ok(Self {
            after_sequence,
            tail,
            follow,
            unit,
            pid,
            max_priority,
        })
    }
}

/// Stored record returned by `logd`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogRecord {
    /// Strictly increasing sequence number within this log daemon session.
    pub sequence: u64,
    /// Boot-relative monotonic timestamp in nanoseconds.
    pub monotonic_ns: u64,
    /// Unix epoch timestamp in nanoseconds, or `u64::MAX` when unavailable.
    pub realtime_ns: u64,
    /// Identifier for the current in-memory boot journal.
    pub boot_id: u64,
    /// Service or application identity.
    pub unit: String,
    /// Originating process ID.
    pub pid: i32,
    /// Origin stream.
    pub stream: LogStream,
    /// Record priority.
    pub priority: LogPriority,
    /// Uninterpreted log message bytes.
    pub message: Vec<u8>,
}

impl LogRecord {
    /// Encode this stored record.
    ///
    /// # Returns
    ///
    /// The payload bytes, or an error when a field exceeds protocol limits.
    pub fn to_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        validate_text_fields(self.unit.as_bytes(), &self.message)?;
        let mut out = Vec::with_capacity(48 + self.unit.len() + self.message.len());
        push_u64(&mut out, self.sequence);
        push_u64(&mut out, self.monotonic_ns);
        push_u64(&mut out, self.realtime_ns);
        push_u64(&mut out, self.boot_id);
        push_i32(&mut out, self.pid);
        out.push(self.stream.as_u8());
        out.push(self.priority.as_u8());
        out.extend_from_slice(&0u16.to_le_bytes());
        push_u32(&mut out, self.unit.len() as u32);
        push_u32(&mut out, self.message.len() as u32);
        out.extend_from_slice(self.unit.as_bytes());
        out.extend_from_slice(&self.message);
        validate_payload_size(&out)?;
        Ok(out)
    }

    /// Decode a stored record payload.
    ///
    /// # Arguments
    ///
    /// * `payload` - Bytes following a [`MSG_RECORD`] header.
    ///
    /// # Returns
    ///
    /// The decoded record, or a protocol error.
    pub fn from_payload(payload: &[u8]) -> Result<Self, ProtocolError> {
        validate_payload_size(payload)?;
        let mut cursor = Cursor::new(payload);
        let sequence = cursor.u64()?;
        let monotonic_ns = cursor.u64()?;
        let realtime_ns = cursor.u64()?;
        let boot_id = cursor.u64()?;
        let pid = cursor.i32()?;
        let stream = LogStream::from_u8(cursor.u8()?).ok_or(ProtocolError::InvalidValue)?;
        let priority = LogPriority::from_u8(cursor.u8()?).ok_or(ProtocolError::InvalidValue)?;
        if cursor.u16()? != 0 {
            return Err(ProtocolError::InvalidValue);
        }
        let unit_len = cursor.u32()? as usize;
        let message_len = cursor.u32()? as usize;
        if unit_len > MAX_UNIT_LEN || message_len > MAX_MESSAGE_LEN {
            return Err(ProtocolError::InvalidLength);
        }
        let unit = decode_unit(cursor.bytes(unit_len)?)?;
        let message = cursor.bytes(message_len)?.to_vec();
        cursor.finish()?;
        Ok(Self {
            sequence,
            monotonic_ns,
            realtime_ns,
            boot_id,
            unit,
            pid,
            stream,
            priority,
            message,
        })
    }
}

/// Marker terminating a finite query response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryEnd {
    /// Last sequence allocated by `logd` when the snapshot completed.
    pub last_sequence: u64,
    /// Identifier for the current in-memory boot journal.
    pub boot_id: u64,
}

impl QueryEnd {
    /// Encode this query terminator.
    ///
    /// # Returns
    ///
    /// The fixed-width payload bytes.
    pub fn to_payload(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..8].copy_from_slice(&self.last_sequence.to_le_bytes());
        out[8..16].copy_from_slice(&self.boot_id.to_le_bytes());
        out
    }

    /// Decode a query terminator.
    ///
    /// # Arguments
    ///
    /// * `payload` - Bytes following a [`MSG_QUERY_END`] header.
    ///
    /// # Returns
    ///
    /// The decoded marker, or an error for a non-16-byte payload.
    pub fn from_payload(payload: &[u8]) -> Result<Self, ProtocolError> {
        if payload.len() != 16 {
            return Err(ProtocolError::InvalidLength);
        }
        Ok(Self {
            last_sequence: u64::from_le_bytes([
                payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
                payload[7],
            ]),
            boot_id: u64::from_le_bytes([
                payload[8],
                payload[9],
                payload[10],
                payload[11],
                payload[12],
                payload[13],
                payload[14],
                payload[15],
            ]),
        })
    }
}

fn validate_payload_size(payload: &[u8]) -> Result<(), ProtocolError> {
    if payload.len() > MAX_PAYLOAD_SIZE {
        Err(ProtocolError::InvalidLength)
    } else {
        Ok(())
    }
}

fn validate_text_fields(unit: &[u8], message: &[u8]) -> Result<(), ProtocolError> {
    if unit.len() > MAX_UNIT_LEN || message.len() > MAX_MESSAGE_LEN {
        return Err(ProtocolError::InvalidLength);
    }
    core::str::from_utf8(unit).map_err(|_| ProtocolError::InvalidValue)?;
    Ok(())
}

fn decode_unit(bytes: &[u8]) -> Result<String, ProtocolError> {
    core::str::from_utf8(bytes)
        .map(String::from)
        .map_err(|_| ProtocolError::InvalidValue)
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ProtocolError::InvalidLength)?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or(ProtocolError::Truncated)?;
        self.offset = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtocolError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i32(&mut self) -> Result<i32, ProtocolError> {
        let bytes = self.bytes(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        let bytes = self.bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn finish(self) -> Result<(), ProtocolError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(ProtocolError::TrailingBytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_round_trip_preserves_binary_message() {
        let request = AppendRequest {
            unit: String::from("sws"),
            pid: 42,
            stream: LogStream::Stderr,
            priority: LogPriority::Warning,
            message: vec![0xff, b'x', 0],
        };
        let encoded = request.to_payload().unwrap();
        assert_eq!(AppendRequest::from_payload(&encoded).unwrap(), request);
    }

    #[test]
    fn query_round_trip_preserves_filters() {
        let query = Query {
            after_sequence: 93,
            tail: 200,
            follow: true,
            unit: String::from("sws"),
            pid: ANY_PID,
            max_priority: LogPriority::Warning.as_u8(),
        };
        let encoded = query.to_payload().unwrap();
        assert_eq!(Query::from_payload(&encoded).unwrap(), query);
    }

    #[test]
    fn record_round_trip_preserves_metadata() {
        let record = LogRecord {
            sequence: 10,
            monotonic_ns: 20,
            realtime_ns: u64::MAX,
            boot_id: 30,
            unit: String::from("video-player"),
            pid: 77,
            stream: LogStream::Stdout,
            priority: LogPriority::Info,
            message: b"frame presented".to_vec(),
        };
        let encoded = record.to_payload().unwrap();
        assert_eq!(LogRecord::from_payload(&encoded).unwrap(), record);
    }

    #[test]
    fn truncated_and_trailing_payloads_are_rejected() {
        let request = AppendRequest {
            unit: String::from("sws"),
            pid: 1,
            stream: LogStream::Stdout,
            priority: LogPriority::Info,
            message: b"ready".to_vec(),
        };
        let mut encoded = request.to_payload().unwrap();
        assert_eq!(
            AppendRequest::from_payload(&encoded[..encoded.len() - 1]),
            Err(ProtocolError::Truncated)
        );
        encoded.push(0);
        assert_eq!(
            AppendRequest::from_payload(&encoded),
            Err(ProtocolError::TrailingBytes)
        );
    }

    #[test]
    fn priority_names_are_syslog_compatible() {
        assert_eq!(LogPriority::parse("warning"), Some(LogPriority::Warning));
        assert_eq!(LogPriority::parse("7"), Some(LogPriority::Debug));
        assert_eq!(LogPriority::parse("verbose"), None);
    }
}
