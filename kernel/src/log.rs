//! Kernel log ring buffer (dmesg)
//!
//! Stores all kernel messages in a fixed-size circular buffer so they can be
//! retrieved later by user-space programs (e.g. `/dev/kmsg`).
//!
//! Writes append to the buffer; old data is overwritten when full.
//! Readers wait on `READER_WAKER` until new data arrives.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::sync::waker::Waker;
use crate::sync::{IrqSpinLock, IrqSpinLockGuard};

const LOG_BUF_SIZE: usize = 1 << 18; // 256 KiB
/// Largest single userspace submission accepted by the native log core.
pub(crate) const MAX_USER_MESSAGE_BYTES: usize = 1024;
/// Number of userspace records whose exact PID metadata is retained. A parallel
/// per-byte tag keeps the origin class correct after old PID metadata is
/// evicted, while the byte ring's normal capacity controls text retention.
const MAX_USER_RECORDS: usize = 256;

static BUF: [core::sync::atomic::AtomicU8; LOG_BUF_SIZE] =
    [const { core::sync::atomic::AtomicU8::new(0) }; LOG_BUF_SIZE];
static USER_BYTES: [AtomicU8; LOG_BUF_SIZE] = [const { AtomicU8::new(0) }; LOG_BUF_SIZE];

static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

pub static READER_WAKER: Waker = Waker::new_interruptible("kmsg");

static PRINT_LOCK: IrqSpinLock<()> = IrqSpinLock::new(());
static BYTE_LOCK: IrqSpinLock<()> = IrqSpinLock::new(());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Origin {
    Kernel,
    Userspace { pid: Option<usize> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UserspaceLogError {
    InvalidArgument,
    TooLarge,
    NoCurrentTask,
}

#[derive(Clone, Copy)]
struct UserRecord {
    start: usize,
    end: usize,
    pid: usize,
}

struct UserRecords {
    entries: [UserRecord; MAX_USER_RECORDS],
    head: usize,
    len: usize,
}

impl UserRecords {
    const fn new() -> Self {
        Self {
            entries: [UserRecord {
                start: 0,
                end: 0,
                pid: 0,
            }; MAX_USER_RECORDS],
            head: 0,
            len: 0,
        }
    }

    fn drop_before(&mut self, tail: usize) {
        while self.len != 0 && self.entries[self.head].start < tail {
            self.head = (self.head + 1) % MAX_USER_RECORDS;
            self.len -= 1;
        }
    }

    fn push(&mut self, record: UserRecord) {
        if self.len == MAX_USER_RECORDS {
            self.head = (self.head + 1) % MAX_USER_RECORDS;
            self.len -= 1;
        }
        let index = (self.head + self.len) % MAX_USER_RECORDS;
        self.entries[index] = record;
        self.len += 1;
    }

    fn origin(&self, position: usize) -> Option<Origin> {
        (0..self.len).find_map(|offset| {
            let record = self.entries[(self.head + offset) % MAX_USER_RECORDS];
            (record.start <= position && position < record.end).then_some(Origin::Userspace {
                pid: Some(record.pid),
            })
        })
    }
}

static USER_RECORDS: IrqSpinLock<UserRecords> = IrqSpinLock::new(UserRecords::new());

pub struct PrintGuard {
    _lock: IrqSpinLockGuard<'static, ()>,
}

impl PrintGuard {
    pub fn acquire() -> Self {
        let lock = PRINT_LOCK.lock();
        Self { _lock: lock }
    }
}

// =============================================================================
// Emergency console (lock-free direct UART output)
// =============================================================================
//
// A registered `fn(u8)` emitter that writes directly to the boot UART, bypassing
// `PRINT_LOCK` and the ring buffer. Used by the emergency print path (panic,
// watchdog fatal) where the normal print path may be deadlocked or corrupted.

type EmergencyPutc = fn(u8);

static EMERGENCY_PUTC: AtomicUsize = AtomicUsize::new(0);

/// Register the global emergency byte emitter.
///
/// Should be called exactly once during UART probe. Later registrations are
/// ignored to preserve determinism. The function pointer must remain valid for
/// the entire kernel lifetime (it always does, since it is a static `fn`).
pub fn register_emergency_putc(putc: EmergencyPutc) {
    let addr = putc as usize;
    let _ = EMERGENCY_PUTC.compare_exchange(0, addr, Ordering::Release, Ordering::Relaxed);
}

/// Replace the fallback emitter when the boot-selected console becomes ready.
/// Only console selection may override an earlier registration; unrelated
/// device probes continue using `register_emergency_putc`. Both old and new
/// callbacks must remain valid for the entire kernel lifetime.
pub(crate) fn select_emergency_putc(putc: EmergencyPutc) {
    EMERGENCY_PUTC.store(putc as usize, Ordering::Release);
}

/// Write a single byte through the registered emergency emitter.
///
/// Before UART discovery, RISC-V can still use the firmware's byte console.
/// This path must not acquire the ordinary early-console or log-ring locks.
pub fn emergency_putc(byte: u8) {
    let addr = EMERGENCY_PUTC.load(Ordering::Acquire);
    if addr == 0 {
        #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
        crate::arch::riscv::earlycon::early_putc(byte);
        return;
    }
    // SAFETY: the address was published by `register_emergency_putc` from a
    // static `fn(u8)` pointer and is never cleared. `fn` pointers are plain
    // thin pointers, so transmuting the `usize` back is sound.
    let putc: EmergencyPutc = unsafe { core::mem::transmute(addr) };
    putc(byte);
}

/// Print a formatted message through the normal (locked) path.
///
/// This acquires `PRINT_LOCK`, writes to the ring buffer, and emits to the
/// console. If another CPU is stuck holding the print lock this will spin
/// until it is released. For contexts where that is unacceptable (panic,
/// watchdog fatal), use [`emergency_print`] instead.
pub fn print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    let _guard = PrintGuard::acquire();
    let mut writer = PrintWriter;
    let _ = writer.write_fmt(args);
}

/// Print a formatted message through the emergency (lock-free) path.
///
/// This bypasses `PRINT_LOCK` and the ring buffer entirely and writes directly
/// to the registered emergency UART. Use this from panic handlers, watchdog
/// fatal reports, and any context where the normal print path may be deadlocked
/// or corrupted. Output from multiple CPUs may interleave; the priority is
/// guaranteed visibility over ordering.
pub fn emergency_print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    let mut writer = EmergencyWriter;
    let _ = writer.write_fmt(args);
}

/// Write a string directly to the UART hardware without acquiring the print
/// lock or the log ring buffer.
pub fn emergency_write(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            emergency_putc(b'\r');
        }
        emergency_putc(b);
    }
}

struct EmergencyWriter;

impl core::fmt::Write for EmergencyWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        emergency_write(s);
        Ok(())
    }
}

struct PrintWriter;

impl core::fmt::Write for PrintWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        crate::earlycon::EarlyConsole.write_str(s)
    }
}

pub fn write_bytes(data: &[u8]) {
    let _lock = BYTE_LOCK.lock();
    let head = HEAD.load(Ordering::Relaxed);
    let mut pos = head;

    for &b in data {
        let index = pos % LOG_BUF_SIZE;
        BUF[index].store(b, Ordering::Relaxed);
        USER_BYTES[index].store(0, Ordering::Relaxed);
        pos += 1;
    }

    HEAD.store(pos, Ordering::Release);

    let tail = TAIL.load(Ordering::Relaxed);
    if pos - tail > LOG_BUF_SIZE {
        let new_tail = pos - LOG_BUF_SIZE;
        TAIL.store(new_tail, Ordering::Release);
        USER_RECORDS.lock().drop_before(new_tail);
    }
    drop(_lock);
    READER_WAKER.wake_all();
}

pub fn write_byte(b: u8) {
    let _lock = BYTE_LOCK.lock();
    let pos = HEAD.load(Ordering::Relaxed);
    let index = pos % LOG_BUF_SIZE;
    BUF[index].store(b, Ordering::Relaxed);
    USER_BYTES[index].store(0, Ordering::Relaxed);
    HEAD.store(pos + 1, Ordering::Release);

    let tail = TAIL.load(Ordering::Relaxed);
    if pos + 1 - tail > LOG_BUF_SIZE {
        let new_tail = pos + 1 - LOG_BUF_SIZE;
        TAIL.store(new_tail, Ordering::Release);
        USER_RECORDS.lock().drop_before(new_tail);
    }
    // The formatted print path emits byte-by-byte. Publish line completion so
    // blocking /dev/kmsg readers wake without paying a scheduler wake per byte.
    drop(_lock);
    if b == b'\n' {
        READER_WAKER.wake_all();
    }
}

fn write_user_locked(data: &[u8], severity: u8) -> usize {
    let mut pos = HEAD.load(Ordering::Relaxed);
    let origin = severity.saturating_add(1);
    for &byte in data {
        let index = pos % LOG_BUF_SIZE;
        BUF[index].store(byte, Ordering::Relaxed);
        USER_BYTES[index].store(origin, Ordering::Relaxed);
        pos += 1;
    }
    HEAD.store(pos, Ordering::Release);

    let tail = TAIL.load(Ordering::Relaxed);
    let new_tail = pos.saturating_sub(LOG_BUF_SIZE).max(tail);
    if new_tail != tail {
        TAIL.store(new_tail, Ordering::Release);
        USER_RECORDS.lock().drop_before(new_tail);
    }
    pos
}

fn write_decimal(buffer: &mut [u8], offset: &mut usize, mut value: usize) {
    let mut digits = [0u8; 20];
    let mut cursor = digits.len();
    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let length = digits.len() - cursor;
    buffer[*offset..*offset + length].copy_from_slice(&digits[cursor..]);
    *offset += length;
}

fn submit_userspace_as(pid: usize, level: u8, data: &[u8]) -> Result<usize, UserspaceLogError> {
    if data.is_empty() || level > 7 {
        return Err(UserspaceLogError::InvalidArgument);
    }
    if data.len() > MAX_USER_MESSAGE_BYTES {
        return Err(UserspaceLogError::TooLarge);
    }

    // Hold the same lock as the formatted kernel print path so a user record
    // cannot be interleaved with another record on another CPU.
    let _print_guard = PrintGuard::acquire();
    let _byte_guard = BYTE_LOCK.lock();
    let start = HEAD.load(Ordering::Acquire);
    let mut framed = [0u8; MAX_USER_MESSAGE_BYTES + 64];
    let mut length = 0;
    let marker = b"userspace[pid=";
    framed[length..length + marker.len()].copy_from_slice(marker);
    length += marker.len();
    write_decimal(&mut framed, &mut length, pid);
    framed[length..length + 2].copy_from_slice(b"] ");
    length += 2;
    framed[length..length + data.len()].copy_from_slice(data);
    length += data.len();
    if data.last() != Some(&b'\n') {
        framed[length] = b'\n';
        length += 1;
    }
    write_user_locked(&framed[..length], level);
    let end = HEAD.load(Ordering::Acquire);
    USER_RECORDS.lock().push(UserRecord { start, end, pid });

    drop(_byte_guard);
    drop(_print_guard);
    READER_WAKER.wake_all();
    Ok(data.len())
}

/// Append one complete userspace log submission.
///
/// The caller supplies only the message. The kernel derives the process origin
/// from the current task and constructs the record prefix, so an ABI cannot
/// claim kernel origin or choose an arbitrary source PID.
pub(crate) fn submit_userspace(data: &[u8]) -> Result<usize, UserspaceLogError> {
    submit_userspace_with_level(data, 6)
}

/// Append a userspace submission with a native severity value in the range
/// 0 through 7. The value is carried as plain text; no Linux record layout is
/// introduced into the native log core.
pub(crate) fn submit_userspace_with_level(
    data: &[u8],
    level: u8,
) -> Result<usize, UserspaceLogError> {
    let task = crate::task::mytask().ok_or(UserspaceLogError::NoCurrentTask)?;
    submit_userspace_as(task.get_thread_group_id(), level, data)
}

pub fn read_at(cursor: usize, buf: &mut [u8]) -> usize {
    let _lock = BYTE_LOCK.lock();
    let tail = TAIL.load(Ordering::Acquire);
    let head = HEAD.load(Ordering::Acquire);

    let start = cursor.max(tail);
    let available = head.saturating_sub(start);
    let to_copy = buf.len().min(available);

    for i in 0..to_copy {
        buf[i] = BUF[(start + i) % LOG_BUF_SIZE].load(Ordering::Relaxed);
    }

    to_copy
}

/// Read at most one newline-terminated record without changing any global
/// cursor. The returned tuple is `(bytes_read, next_cursor)`. If the supplied
/// buffer fills first, the remainder of that record is available at the
/// returned cursor.
pub(crate) fn read_record_at(cursor: usize, buf: &mut [u8]) -> (usize, usize) {
    let _lock = BYTE_LOCK.lock();
    let tail = TAIL.load(Ordering::Acquire);
    let head = HEAD.load(Ordering::Acquire);
    let start = cursor.max(tail);
    if start >= head || buf.is_empty() {
        return (0, start);
    }

    let limit = buf.len().min(head - start);
    let mut count = 0;
    while count < limit {
        let byte = BUF[(start + count) % LOG_BUF_SIZE].load(Ordering::Relaxed);
        buf[count] = byte;
        count += 1;
        if byte == b'\n' {
            break;
        }
    }
    (count, start + count)
}

/// A copied line fragment and its native metadata from the same ring snapshot.
pub(crate) struct RecordSlice {
    pub start: usize,
    pub next: usize,
    pub length: usize,
    pub origin: Origin,
    pub severity: Option<u8>,
}

/// Copy up to a newline, buffer boundary, or change in origin/severity. Holding
/// the byte lock for both data and metadata avoids separately sampling bytes
/// which may have been overwritten. PID lookup occurs once per fragment.
pub(crate) fn read_record_with_metadata(cursor: usize, buf: &mut [u8]) -> Option<RecordSlice> {
    let _lock = BYTE_LOCK.lock();
    let start = cursor.max(TAIL.load(Ordering::Acquire));
    let head = HEAD.load(Ordering::Acquire);
    if start >= head || buf.is_empty() {
        return None;
    }
    let tag = USER_BYTES[start % LOG_BUF_SIZE].load(Ordering::Relaxed);
    let origin = if tag == 0 {
        Origin::Kernel
    } else {
        USER_RECORDS
            .lock()
            .origin(start)
            .unwrap_or(Origin::Userspace { pid: None })
    };
    let limit = buf.len().min(head - start);
    let mut length = 0;
    while length < limit {
        let index = (start + length) % LOG_BUF_SIZE;
        if USER_BYTES[index].load(Ordering::Relaxed) != tag {
            break;
        }
        let byte = BUF[index].load(Ordering::Relaxed);
        buf[length] = byte;
        length += 1;
        if byte == b'\n' {
            break;
        }
    }
    Some(RecordSlice {
        start,
        next: start + length,
        length,
        origin,
        severity: tag.checked_sub(1),
    })
}

pub(crate) fn has_data(cursor: usize) -> bool {
    let tail = TAIL.load(Ordering::Acquire);
    let head = HEAD.load(Ordering::Acquire);
    cursor.max(tail) < head
}

pub(crate) fn origin_at(position: usize) -> Option<Origin> {
    let _lock = BYTE_LOCK.lock();
    let tail = TAIL.load(Ordering::Acquire);
    let head = HEAD.load(Ordering::Acquire);
    if position < tail || position >= head {
        return None;
    }
    if USER_BYTES[position % LOG_BUF_SIZE].load(Ordering::Acquire) == 0 {
        return Some(Origin::Kernel);
    }
    Some(
        USER_RECORDS
            .lock()
            .origin(position)
            .unwrap_or(Origin::Userspace { pid: None }),
    )
}

/// Return the native severity recorded with the userspace byte at `position`.
/// Kernel print bytes and positions evicted from the ring have no severity.
pub(crate) fn severity_at(position: usize) -> Option<u8> {
    let tail = TAIL.load(Ordering::Acquire);
    let head = HEAD.load(Ordering::Acquire);
    if position < tail || position >= head {
        return None;
    }
    let origin = USER_BYTES[position % LOG_BUF_SIZE].load(Ordering::Acquire);
    origin.checked_sub(1)
}

pub(crate) fn clear() {
    let _print_guard = PrintGuard::acquire();
    let _byte_guard = BYTE_LOCK.lock();
    let head = HEAD.load(Ordering::Acquire);
    TAIL.store(head, Ordering::Release);
    let mut records = USER_RECORDS.lock();
    records.head = 0;
    records.len = 0;
}

pub(crate) const fn capacity() -> usize {
    LOG_BUF_SIZE
}

#[cfg(test)]
pub(crate) fn submit_userspace_for_test(
    pid: usize,
    level: u8,
    data: &[u8],
) -> Result<usize, UserspaceLogError> {
    submit_userspace_as(pid, level, data)
}

pub fn head() -> usize {
    HEAD.load(Ordering::Acquire)
}

pub fn tail() -> usize {
    TAIL.load(Ordering::Acquire)
}

pub fn len() -> usize {
    let head = HEAD.load(Ordering::Acquire);
    let tail = TAIL.load(Ordering::Acquire);
    head - tail
}

pub fn is_empty() -> bool {
    len() == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn userspace_record_uses_kernel_derived_origin_and_stays_in_the_text_ring() {
        let start = head();
        assert_eq!(
            submit_userspace_as(4242, 6, b"userspace-origin-token"),
            Ok(22)
        );

        let mut record = [0u8; 128];
        let (length, next) = read_record_at(start, &mut record);
        assert_eq!(
            &record[..length],
            b"userspace[pid=4242] userspace-origin-token\n"
        );
        assert_eq!(next, start + length);
        assert_eq!(
            origin_at(start),
            Some(Origin::Userspace { pid: Some(4242) })
        );
        assert_eq!(severity_at(start), Some(6));
    }

    #[test_case]
    fn kernel_print_bytes_keep_their_existing_unframed_stream() {
        let start = head();
        write_byte(b'K');

        let mut byte = [0u8; 1];
        assert_eq!(read_at(start, &mut byte), 1);
        assert_eq!(byte, *b"K");
        assert_eq!(origin_at(start), Some(Origin::Kernel));
    }

    #[test_case]
    fn log_reader_ahead_of_the_writer_has_no_bytes() {
        let mut output = [0u8; 8];
        assert_eq!(read_at(head() + 1, &mut output), 0);
    }

    #[test_case]
    fn metadata_snapshot_stops_before_a_different_origin_and_keeps_copied_metadata() {
        let start = head();
        write_bytes(b"partial kernel");
        submit_userspace_as(31, 3, b"first\nsecond").unwrap();
        let mut output = [0u8; 128];
        let kernel = read_record_with_metadata(start, &mut output).unwrap();
        assert_eq!(&output[..kernel.length], b"partial kernel");
        assert_eq!(kernel.origin, Origin::Kernel);
        assert_eq!(kernel.severity, None);
        assert_eq!(kernel.next, start + kernel.length);
        let user = read_record_with_metadata(kernel.next, &mut output).unwrap();
        assert_eq!(&output[..user.length], b"userspace[pid=31] first\n");
        assert_eq!(user.origin, Origin::Userspace { pid: Some(31) });
        assert_eq!(user.severity, Some(3));
        let continuation = read_record_with_metadata(user.next, &mut output).unwrap();
        assert_eq!(&output[..continuation.length], b"second\n");
        assert_eq!(continuation.origin, user.origin);
        assert_eq!(continuation.severity, user.severity);
        for sequence in 0..=MAX_USER_RECORDS {
            submit_userspace_as(sequence, 6, b"metadata-retention").unwrap();
        }
        assert_eq!(user.origin, Origin::Userspace { pid: Some(31) });
        let retained = read_record_with_metadata(kernel.next, &mut output).unwrap();
        assert_eq!(retained.origin, Origin::Userspace { pid: None });
        assert_eq!(retained.severity, Some(3));
    }

    #[test_case]
    fn oversized_userspace_record_is_rejected_without_advancing_the_queue() {
        let before = head();
        let oversized = [b'x'; MAX_USER_MESSAGE_BYTES + 1];
        assert_eq!(
            submit_userspace_as(7, 6, &oversized),
            Err(UserspaceLogError::TooLarge)
        );
        assert_eq!(head(), before);
    }

    #[test_case]
    fn record_metadata_eviction_never_reclassifies_retained_user_bytes() {
        let first = head();
        for sequence in 0..=MAX_USER_RECORDS {
            submit_userspace_as(sequence, 6, b"bounded-origin-token").unwrap();
        }

        assert_eq!(
            origin_at(first),
            Some(Origin::Userspace { pid: None }),
            "per-byte origin survives eviction from the bounded metadata queue"
        );
    }
}
