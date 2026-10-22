//! Finite logctl-compatible queries keep cancellation and memory use bounded.

use super::model::{Entry, MAX_RECORDS};
use log_protocol::{
    HEADER_SIZE, Header, LogRecord, MAX_PAYLOAD_SIZE, MSG_ERROR, MSG_QUERY_END, MSG_RECORD,
    QueryEnd,
};
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub end: QueryEnd,
}

#[cfg(target_os = "scarlet")]
pub fn query(after_sequence: u64, cancelled: impl Fn() -> bool) -> Result<Snapshot, String> {
    use log_protocol::{ANY_PID, ANY_PRIORITY, MSG_QUERY, Query, SOCKET_PATH};
    use scarlet_os::socket::Socket;
    let mut socket =
        Socket::new().map_err(|error| format!("Cannot create log socket: {error:?}"))?;
    socket
        .connect(SOCKET_PATH)
        .map_err(|error| format!("Cannot connect to logd: {error:?}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|error| format!("Cannot configure log socket: {error:?}"))?;
    let payload = Query {
        after_sequence,
        tail: MAX_RECORDS as u32,
        follow: false,
        unit: String::new(),
        pid: ANY_PID,
        max_priority: ANY_PRIORITY,
    }
    .to_payload()
    .map_err(|error| format!("Invalid log query: {error:?}"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let header = Header {
        message_type: MSG_QUERY,
        payload_size: payload.len() as u32,
    }
    .to_le_bytes();
    write_until(&mut socket, &header, deadline, &cancelled)?;
    write_until(&mut socket, &payload, deadline, &cancelled)?;
    read_snapshot(&mut socket, deadline, &cancelled)
}

#[cfg(not(target_os = "scarlet"))]
pub fn query(_after_sequence: u64, _cancelled: impl Fn() -> bool) -> Result<Snapshot, String> {
    Err("System logs require Scarlet OS. Host preview: launch with --preview.".into())
}

fn check_deadline(deadline: Instant, cancelled: &impl Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        return Err("Query cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("logd query timed out; use Refresh to retry.".into());
    }
    Ok(())
}

#[cfg_attr(not(target_os = "scarlet"), allow(dead_code))]
fn write_until(
    stream: &mut impl Write,
    mut bytes: &[u8],
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    while !bytes.is_empty() {
        check_deadline(deadline, cancelled)?;
        match stream.write(bytes) {
            Ok(0) => return Err("logd closed the connection while sending a query.".into()),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(format!("Cannot send log query: {error}")),
        }
    }
    Ok(())
}

fn read_until(
    stream: &mut impl Read,
    mut bytes: &mut [u8],
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    while !bytes.is_empty() {
        check_deadline(deadline, cancelled)?;
        match stream.read(bytes) {
            Ok(0) => return Err("logd closed the connection before completing the query.".into()),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(format!("Cannot read logs: {error}")),
        }
    }
    Ok(())
}

#[cfg_attr(not(target_os = "scarlet"), allow(dead_code))]
fn read_snapshot(
    stream: &mut impl Read,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<Snapshot, String> {
    let mut entries = Vec::new();
    loop {
        let mut bytes = [0; HEADER_SIZE];
        read_until(stream, &mut bytes, deadline, cancelled)?;
        let header = Header::from_le_bytes(bytes);
        if header.payload_size as usize > MAX_PAYLOAD_SIZE {
            return Err("logd sent an oversized frame.".into());
        }
        let mut payload = vec![0; header.payload_size as usize];
        read_until(stream, &mut payload, deadline, cancelled)?;
        match header.message_type {
            MSG_RECORD => {
                if entries.len() == MAX_RECORDS {
                    return Err("logd exceeded the requested history limit.".into());
                }
                let record = LogRecord::from_payload(&payload)
                    .map_err(|error| format!("Invalid log record: {error:?}"))?;
                entries.push(Entry::from_record(record));
            }
            MSG_QUERY_END => {
                return Ok(Snapshot {
                    entries,
                    end: QueryEnd::from_payload(&payload)
                        .map_err(|error| format!("Invalid log query terminator: {error:?}"))?,
                });
            }
            MSG_ERROR => {
                return Err(format!(
                    "logd: {}",
                    String::from_utf8_lossy(&payload)
                        .chars()
                        .take(160)
                        .collect::<String>()
                ));
            }
            _ => return Err("logd sent an unexpected response.".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn frame(kind: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Header {
            message_type: kind,
            payload_size: payload.len() as u32,
        }
        .to_le_bytes()
        .to_vec();
        bytes.extend(payload);
        bytes
    }
    fn read(bytes: Vec<u8>) -> Result<Snapshot, String> {
        read_snapshot(
            &mut Cursor::new(bytes),
            Instant::now() + Duration::from_secs(1),
            &|| false,
        )
    }

    #[test]
    fn finite_query_requires_a_valid_end_marker() {
        let snapshot = read(frame(
            MSG_QUERY_END,
            &QueryEnd {
                last_sequence: 17,
                boot_id: 8,
            }
            .to_payload(),
        ))
        .unwrap();
        assert_eq!(snapshot.end.last_sequence, 17);
        assert!(snapshot.entries.is_empty());
        assert!(read(vec![]).is_err());
        assert!(read(frame(MSG_QUERY_END, &[0; 15])).is_err());
    }

    #[test]
    fn malformed_and_oversized_frames_are_rejected() {
        assert!(read(frame(MSG_RECORD, b"bad record")).is_err());
        assert!(read(frame(0x5555, &[])).is_err());
        let bytes = Header {
            message_type: MSG_RECORD,
            payload_size: MAX_PAYLOAD_SIZE as u32 + 1,
        }
        .to_le_bytes()
        .to_vec();
        assert!(read(bytes).unwrap_err().contains("oversized"));
    }

    #[test]
    fn cancellation_and_timeout_are_checked_before_io() {
        let mut bytes = Cursor::new(vec![]);
        assert!(
            read_snapshot(&mut bytes, Instant::now() + Duration::from_secs(1), &|| {
                true
            })
            .unwrap_err()
            .contains("cancelled")
        );
        assert!(
            read_snapshot(&mut bytes, Instant::now(), &|| false)
                .unwrap_err()
                .contains("timed out")
        );
    }

    #[test]
    fn partial_reads_and_a_temporary_would_block_preserve_record_framing() {
        use log_protocol::{LogPriority, LogStream};
        struct Chunks {
            bytes: Cursor<Vec<u8>>,
            block_once: bool,
        }
        impl Read for Chunks {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                if self.block_once {
                    self.block_once = false;
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
                let len = output.len().min(3);
                self.bytes.read(&mut output[..len])
            }
        }
        let record = LogRecord {
            sequence: 7,
            boot_id: 2,
            monotonic_ns: 900,
            realtime_ns: u64::MAX,
            unit: "service".into(),
            pid: 12,
            stream: LogStream::Stdout,
            priority: LogPriority::Info,
            message: b"complete message".to_vec(),
        };
        let mut bytes = frame(MSG_RECORD, &record.to_payload().unwrap());
        bytes.extend(frame(
            MSG_QUERY_END,
            &QueryEnd {
                last_sequence: 7,
                boot_id: 2,
            }
            .to_payload(),
        ));
        let snapshot = read_snapshot(
            &mut Chunks {
                bytes: Cursor::new(bytes),
                block_once: true,
            },
            Instant::now() + Duration::from_secs(1),
            &|| false,
        )
        .unwrap();
        assert_eq!(snapshot.entries[0], Entry::from_record(record));
        assert_eq!(snapshot.end.boot_id, 2);
    }
}
