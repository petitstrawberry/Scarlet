//! Central runtime log collector for Scarlet services and applications.

use log_protocol::{ANY_PID, ANY_PRIORITY, AppendRequest, LogRecord, Query};
use std::collections::VecDeque;

const MAX_RECORDS: usize = 8_192;
const MAX_STORED_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
struct LogStore {
    records: VecDeque<LogRecord>,
    stored_bytes: usize,
    next_sequence: u64,
    boot_id: u64,
}

impl LogStore {
    fn new(boot_id: u64) -> Self {
        Self {
            records: VecDeque::new(),
            stored_bytes: 0,
            next_sequence: 1,
            boot_id,
        }
    }

    fn append(
        &mut self,
        request: AppendRequest,
        monotonic_ns: u64,
        realtime_ns: Option<u64>,
    ) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1).max(1);
        let record = LogRecord {
            sequence,
            monotonic_ns,
            realtime_ns: realtime_ns.unwrap_or(u64::MAX),
            boot_id: self.boot_id,
            unit: request.unit,
            pid: request.pid,
            stream: request.stream,
            priority: request.priority,
            message: request.message,
        };
        self.stored_bytes = self.stored_bytes.saturating_add(record_size(&record));
        self.records.push_back(record);
        self.enforce_limits();
        sequence
    }

    fn query(&self, query: &Query, apply_tail: bool) -> Vec<LogRecord> {
        let mut records: Vec<LogRecord> = self
            .records
            .iter()
            .filter(|record| record.sequence > query.after_sequence)
            .filter(|record| query.unit.is_empty() || record.unit == query.unit)
            .filter(|record| query.pid == ANY_PID || record.pid == query.pid)
            .filter(|record| {
                query.max_priority == ANY_PRIORITY || record.priority.as_u8() <= query.max_priority
            })
            .cloned()
            .collect();
        if apply_tail && query.tail != 0 {
            let tail = query.tail as usize;
            if records.len() > tail {
                records.drain(..records.len() - tail);
            }
        }
        records
    }

    fn last_sequence(&self) -> u64 {
        self.next_sequence.saturating_sub(1)
    }

    fn enforce_limits(&mut self) {
        while self.records.len() > MAX_RECORDS || self.stored_bytes > MAX_STORED_BYTES {
            let Some(record) = self.records.pop_front() else {
                self.stored_bytes = 0;
                break;
            };
            self.stored_bytes = self.stored_bytes.saturating_sub(record_size(&record));
        }
    }
}

fn record_size(record: &LogRecord) -> usize {
    record
        .unit
        .len()
        .saturating_add(record.message.len())
        .saturating_add(64)
}

#[cfg(target_os = "scarlet")]
mod runtime {
    use super::LogStore;
    use core::time::Duration;
    use log_protocol::{
        AppendRequest, HEADER_SIZE, Header, LogPriority, LogStream, MAX_PAYLOAD_SIZE, MSG_APPEND,
        MSG_ERROR, MSG_QUERY, MSG_QUERY_END, MSG_RECORD, Query, QueryEnd, SOCKET_PATH,
    };
    use scarlet_os::socket::Socket;
    use scarlet_os::time::{monotonic_time_ns, system_time_ns};
    use std::io::{Read, Write};
    use std::process;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::vec::Vec;

    const STEMD_SOCKET_PATH: &str = "/tmp/stemd.sock";
    const STEMD_SERVICE_READY: u8 = 0x06;
    const READY_NOTIFY_ATTEMPTS: usize = 100;
    const READY_NOTIFY_DELAY_MS: u64 = 20;
    const FOLLOW_POLL_MS: u64 = 50;

    pub(super) fn run() -> Result<(), &'static str> {
        let server = Socket::new().map_err(|_| "failed to create log socket")?;
        server
            .bind(SOCKET_PATH)
            .map_err(|_| "failed to bind log socket")?;
        server
            .listen(32)
            .map_err(|_| "failed to listen on log socket")?;

        let boot_id = system_time_ns()
            .unwrap_or_else(monotonic_time_ns)
            .rotate_left(17)
            ^ ((process::id() as u64) << 32)
            ^ monotonic_time_ns();
        let store = Arc::new(Mutex::new(LogStore::new(boot_id.max(1))));
        append_internal_ready(&store);
        notify_service_ready();

        loop {
            let client = match server.accept() {
                Ok(client) => client,
                Err(_) => {
                    thread::yield_now();
                    continue;
                }
            };
            let client_store = Arc::clone(&store);
            if thread::Builder::new()
                .name(String::from("logd-client"))
                .spawn(move || handle_client(client, client_store))
                .is_err()
            {
                append_internal_warning(&store, b"failed to start client worker".to_vec());
            }
        }
    }

    fn append_internal_ready(store: &Arc<Mutex<LogStore>>) {
        let request = AppendRequest {
            unit: String::from("logd"),
            pid: process::id() as i32,
            stream: LogStream::Internal,
            priority: LogPriority::Info,
            message: b"runtime log collector ready".to_vec(),
        };
        store.lock().expect("logd store mutex poisoned").append(
            request,
            monotonic_time_ns(),
            system_time_ns(),
        );
    }

    fn append_internal_warning(store: &Arc<Mutex<LogStore>>, message: Vec<u8>) {
        let request = AppendRequest {
            unit: String::from("logd"),
            pid: process::id() as i32,
            stream: LogStream::Internal,
            priority: LogPriority::Warning,
            message,
        };
        store.lock().expect("logd store mutex poisoned").append(
            request,
            monotonic_time_ns(),
            system_time_ns(),
        );
    }

    fn handle_client(mut client: Socket, store: Arc<Mutex<LogStore>>) {
        loop {
            let Some((header, payload)) = read_frame(&mut client) else {
                return;
            };
            match header.message_type {
                MSG_APPEND => {
                    let Ok(request) = AppendRequest::from_payload(&payload) else {
                        let _ = write_error(&mut client, "invalid append payload");
                        return;
                    };
                    store.lock().expect("logd store mutex poisoned").append(
                        request,
                        monotonic_time_ns(),
                        system_time_ns(),
                    );
                }
                MSG_QUERY => {
                    let Ok(query) = Query::from_payload(&payload) else {
                        let _ = write_error(&mut client, "invalid query payload");
                        return;
                    };
                    serve_query(&mut client, &store, query);
                    return;
                }
                _ => {
                    let _ = write_error(&mut client, "unsupported message type");
                    return;
                }
            }
        }
    }

    fn serve_query(client: &mut Socket, store: &Arc<Mutex<LogStore>>, mut query: Query) {
        let mut apply_tail = true;
        loop {
            let (records, last_sequence, boot_id) = {
                let store = store.lock().expect("logd store mutex poisoned");
                (
                    store.query(&query, apply_tail),
                    store.last_sequence(),
                    store.boot_id,
                )
            };

            for record in records {
                query.after_sequence = query.after_sequence.max(record.sequence);
                let Ok(payload) = record.to_payload() else {
                    continue;
                };
                if write_frame(client, MSG_RECORD, &payload).is_err() {
                    return;
                }
            }

            if !query.follow {
                let end = QueryEnd {
                    last_sequence,
                    boot_id,
                }
                .to_payload();
                let _ = write_frame(client, MSG_QUERY_END, &end);
                return;
            }

            apply_tail = false;
            thread::sleep(Duration::from_millis(FOLLOW_POLL_MS));
        }
    }

    fn read_frame(stream: &mut Socket) -> Option<(Header, Vec<u8>)> {
        let mut header_bytes = [0u8; HEADER_SIZE];
        stream.read_exact(&mut header_bytes).ok()?;
        let header = Header::from_le_bytes(header_bytes);
        let payload_size = header.payload_size as usize;
        if payload_size > MAX_PAYLOAD_SIZE {
            return None;
        }
        let mut payload = vec![0u8; payload_size];
        stream.read_exact(&mut payload).ok()?;
        Some((header, payload))
    }

    fn write_frame(stream: &mut Socket, message_type: u32, payload: &[u8]) -> std::io::Result<()> {
        if payload.len() > MAX_PAYLOAD_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "log payload exceeds protocol limit",
            ));
        }
        let header = Header {
            message_type,
            payload_size: payload.len() as u32,
        };
        stream.write_all(&header.to_le_bytes())?;
        stream.write_all(payload)
    }

    fn write_error(stream: &mut Socket, message: &str) -> std::io::Result<()> {
        write_frame(stream, MSG_ERROR, message.as_bytes())
    }

    fn notify_service_ready() {
        for _ in 0..READY_NOTIFY_ATTEMPTS {
            if let Ok(mut socket) = Socket::new()
                && socket.connect(STEMD_SOCKET_PATH).is_ok()
            {
                let service_name = b"logd";
                let mut payload = Vec::with_capacity(5 + service_name.len());
                payload.push(STEMD_SERVICE_READY);
                payload.extend_from_slice(&(service_name.len() as u32).to_le_bytes());
                payload.extend_from_slice(service_name);
                if socket.write_all(&payload).is_ok() {
                    let mut response = [0u8; 32];
                    if let Ok(length) = socket.read(&mut response)
                        && response[..length].starts_with(b"OK:")
                    {
                        return;
                    }
                }
            }
            thread::sleep(Duration::from_millis(READY_NOTIFY_DELAY_MS));
        }
    }
}

#[cfg(target_os = "scarlet")]
fn main() -> std::process::ExitCode {
    match runtime::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("logd: {error}");
            std::process::ExitCode::from(1)
        }
    }
}

#[cfg(not(target_os = "scarlet"))]
fn main() {
    eprintln!("logd is only available on Scarlet OS");
}

#[cfg(test)]
mod tests {
    use super::*;
    use log_protocol::{LogPriority, LogStream};

    fn request(unit: &str, pid: i32, priority: LogPriority, message: &str) -> AppendRequest {
        AppendRequest {
            unit: String::from(unit),
            pid,
            stream: LogStream::Stdout,
            priority,
            message: message.as_bytes().to_vec(),
        }
    }

    fn all_query() -> Query {
        Query {
            after_sequence: 0,
            tail: 0,
            follow: false,
            unit: String::new(),
            pid: ANY_PID,
            max_priority: ANY_PRIORITY,
        }
    }

    #[test]
    fn query_filters_unit_pid_priority_and_sequence() {
        let mut store = LogStore::new(9);
        store.append(request("sws", 10, LogPriority::Info, "one"), 1, None);
        store.append(request("sws", 11, LogPriority::Warning, "two"), 2, Some(20));
        store.append(request("sas", 11, LogPriority::Error, "three"), 3, None);

        let mut query = all_query();
        query.unit = String::from("sws");
        query.pid = 11;
        query.max_priority = LogPriority::Warning.as_u8();
        query.after_sequence = 1;
        let records = store.query(&query, true);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].message, b"two");
        assert_eq!(records[0].realtime_ns, 20);
    }

    #[test]
    fn tail_returns_newest_matching_records_in_order() {
        let mut store = LogStore::new(1);
        for index in 0..5 {
            store.append(
                request("sws", 1, LogPriority::Info, &index.to_string()),
                index,
                None,
            );
        }
        let mut query = all_query();
        query.tail = 2;
        let records = store.query(&query, true);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].message, b"3");
        assert_eq!(records[1].message, b"4");
    }

    #[test]
    fn bounded_store_evicts_oldest_records() {
        let mut store = LogStore::new(1);
        for index in 0..MAX_RECORDS + 2 {
            store.append(
                request("unit", 1, LogPriority::Info, &index.to_string()),
                index as u64,
                None,
            );
        }
        assert_eq!(store.records.len(), MAX_RECORDS);
        assert_eq!(store.records.front().unwrap().sequence, 3);
        assert_eq!(store.last_sequence(), (MAX_RECORDS + 2) as u64);
    }
}
