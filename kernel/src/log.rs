//! Kernel log ring buffer (dmesg)
//!
//! Stores all kernel messages in a fixed-size circular buffer so they can be
//! retrieved later by user-space programs (e.g. `/dev/kmsg`).
//!
//! Writes append to the buffer; old data is overwritten when full.
//! Readers block via `READER_WAKER` until new data arrives (Linux /dev/kmsg semantics).

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::earlycon::EarlyConsole;
use crate::sync::waker::Waker;
use crate::sync::{IrqSpinLock, IrqSpinLockGuard};

const LOG_BUF_SIZE: usize = 1 << 18; // 256 KiB

static BUF: [core::sync::atomic::AtomicU8; LOG_BUF_SIZE] =
    [const { core::sync::atomic::AtomicU8::new(0) }; LOG_BUF_SIZE];

static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

pub static READER_WAKER: Waker = Waker::new_interruptible("kmsg");

static PRINT_LOCK: IrqSpinLock<()> = IrqSpinLock::new(());

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

/// Write a single byte through the registered emergency emitter.
///
/// No-op when no emitter has been registered (e.g. very early boot or a
/// platform without a UART driver).
pub fn emergency_putc(byte: u8) {
    let addr = EMERGENCY_PUTC.load(Ordering::Acquire);
    if addr == 0 {
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
    let head = HEAD.load(Ordering::Relaxed);
    let mut pos = head;

    for &b in data {
        BUF[pos % LOG_BUF_SIZE].store(b, Ordering::Relaxed);
        pos += 1;
    }

    HEAD.store(pos, Ordering::Release);
    READER_WAKER.wake_all();

    let tail = TAIL.load(Ordering::Relaxed);
    if pos - tail > LOG_BUF_SIZE {
        TAIL.store(pos - LOG_BUF_SIZE, Ordering::Release);
    }
}

pub fn write_byte(b: u8) {
    let pos = HEAD.fetch_add(1, Ordering::Relaxed);
    BUF[pos % LOG_BUF_SIZE].store(b, Ordering::Relaxed);
    // READER_WAKER.wake_all();

    let tail = TAIL.load(Ordering::Relaxed);
    if pos + 1 - tail > LOG_BUF_SIZE {
        TAIL.store(pos + 1 - LOG_BUF_SIZE, Ordering::Release);
    }
}

pub fn read_at(cursor: usize, buf: &mut [u8]) -> usize {
    let tail = TAIL.load(Ordering::Acquire);
    let head = HEAD.load(Ordering::Acquire);

    let start = cursor.max(tail);
    let available = head - start;
    let to_copy = buf.len().min(available);

    for i in 0..to_copy {
        buf[i] = BUF[(start + i) % LOG_BUF_SIZE].load(Ordering::Relaxed);
    }

    to_copy
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
