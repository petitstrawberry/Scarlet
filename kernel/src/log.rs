//! Kernel log ring buffer (dmesg)
//!
//! Stores all kernel messages in a fixed-size circular buffer so they can be
//! retrieved later by user-space programs (e.g. `/dev/kmsg`).
//!
//! Writes append to the buffer; old data is overwritten when full.
//! Readers block via `READER_WAKER` until new data arrives (Linux /dev/kmsg semantics).

use core::sync::atomic::{AtomicUsize, Ordering};

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
