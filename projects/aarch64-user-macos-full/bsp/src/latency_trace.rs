//! Optional host-side latency timeline. Enable only with
//! `SCARLET_HOSTED_LATENCY_TRACE=1` before launching the BSP.
//!
//! `mono_ns` uses mach_absolute_time converted to nanoseconds, the same origin
//! as Swift DispatchTime.uptimeNanoseconds. Records go to host stderr through
//! a bounded, nonblocking channel; the AppKit thread never formats or writes
//! log lines. Disabled hooks do not read the clock or allocate.
//!
//! `key` is the latest key-down observation, NOT an end-to-end frame token:
//! unrelated work can carry the same context. Keyboard/enqueue records use
//! the same key event `id`; GPU receive records use the transport request id;
//! surface records use a frame id. Sort cross-thread records by mono_ns (seq
//! breaks ties). A `lost` record means the timeline is incomplete.
//!
//! Keyboard fields: a=USB HID usage, b=pressed | (repeat << 1).
//! `hid_enqueue_*` brackets InputSender::send (host input queue insertion and
//! IRQ notification), not guest HID consumption. Include its entire bracket
//! when bounding input handoff; the precise insertion is inside that interval.
//! Surface fields: a=physical width, b=physical height. `surface_present_end`
//! means frame.present() returned, not that WindowServer displayed the frame.

use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc::{SyncSender, sync_channel},
};

const FLAG: &str = "SCARLET_HOSTED_LATENCY_TRACE";
const CAPACITY: usize = 4096;
static TRACE: OnceLock<Option<Trace>> = OnceLock::new();

#[repr(C)]
#[derive(Default)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

struct Trace {
    sender: SyncSender<Record>,
    timebase: Timebase,
    sequence: AtomicU64,
    keys: AtomicU64,
    frames: AtomicU64,
    latest_key: AtomicU64,
    lost: std::sync::Arc<AtomicU64>,
}

struct Record {
    seq: u64,
    mono_ns: u64,
    event: &'static str,
    key: u64,
    id: u64,
    a: u64,
    b: u64,
}

/// Called once at BSP window startup, before AppKit starts processing input.
pub(crate) fn init() {
    TRACE.get_or_init(|| {
        if std::env::var_os(FLAG).is_none_or(|value| value != "1") {
            return None;
        }
        let mut timebase = Timebase::default();
        // SAFETY: timebase is exclusively writable for this synchronous call.
        if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
            eprintln!("[hosted-latency] disabled: cannot read Mach timebase");
            return None;
        }
        let (sender, receiver) = sync_channel::<Record>(CAPACITY);
        let lost = std::sync::Arc::new(AtomicU64::new(0));
        let writer_lost = lost.clone();
        if std::thread::Builder::new()
            .name("scarlet-latency".into())
            .spawn(move || {
                use std::{fmt::Write as _, io::Write as _};
                let stderr = std::io::stderr();
                let pid = std::process::id();
                let _ = writeln!(stderr.lock(), "[hosted-latency] v=1 pid={pid} clock=mach_absolute_time_ns key=context_only present=call_return");
                let mut line = String::with_capacity(256);
                while let Ok(record) = receiver.recv() {
                    let dropped = writer_lost.swap(0, Ordering::Relaxed);
                    line.clear();
                    if dropped != 0 {
                        let _ = writeln!(line, "[hosted-latency] v=1 pid={pid} event=lost count={dropped}");
                    }
                    let _ = writeln!(line, "[hosted-latency] v=1 pid={pid} seq={} mono_ns={} event={} key={} id={} a={} b={}",
                        record.seq, record.mono_ns, record.event, record.key, record.id, record.a, record.b);
                    if stderr.lock().write_all(line.as_bytes()).is_err() {
                        break;
                    }
                }
            })
            .is_err()
        {
            eprintln!("[hosted-latency] disabled: cannot start trace writer");
            return None;
        }
        Some(Trace {
            sender,
            timebase,
            sequence: AtomicU64::new(0),
            keys: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            latest_key: AtomicU64::new(0),
            lost,
        })
    });
}

fn active() -> Option<&'static Trace> {
    TRACE.get()?.as_ref()
}

impl Trace {
    fn emit(&self, event: &'static str, key: u64, id: u64, a: u64, b: u64) {
        // SAFETY: mach_absolute_time takes no pointers and is process-wide.
        let ticks = unsafe { mach_absolute_time() };
        let mono_ns = (u128::from(ticks) * u128::from(self.timebase.numer)
            / u128::from(self.timebase.denom)) as u64;
        let record = Record {
            seq: self.sequence.fetch_add(1, Ordering::Relaxed),
            mono_ns,
            event,
            key,
            id,
            a,
            b,
        };
        if self.sender.try_send(record).is_err() {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Key {
    id: u64,
    context: u64,
    usage: u8,
    flags: u64,
}

pub(crate) fn keyboard(usage: u8, pressed: bool, repeat: bool) -> Option<Key> {
    let trace = active()?;
    let id = trace.keys.fetch_add(1, Ordering::Relaxed) + 1;
    if pressed {
        trace.latest_key.store(id, Ordering::Relaxed);
    }
    let key = Key {
        id,
        context: trace.latest_key.load(Ordering::Relaxed),
        usage,
        flags: u64::from(pressed) | (u64::from(repeat) << 1),
    };
    trace.emit(
        "winit_keyboard",
        key.context,
        id,
        u64::from(usage),
        key.flags,
    );
    Some(key)
}

pub(crate) fn hid_enqueue(key: Option<Key>, returned: bool) {
    if let (Some(trace), Some(key)) = (active(), key) {
        trace.emit(
            if returned {
                "hid_enqueue_return"
            } else {
                "hid_enqueue_begin"
            },
            key.context,
            key.id,
            u64::from(key.usage),
            key.flags,
        );
    }
}

/// Command receipts count requested work. Execute records carry the transport
/// opcode and total packet bytes. Image operations carry rectangle dimensions.
pub(crate) fn gpu_receive(id: u64, fields: impl FnOnce() -> Option<(&'static str, u64, u64)>) {
    if let Some(trace) = active() {
        if let Some((event, a, b)) = fields() {
            trace.emit(event, trace.latest_key.load(Ordering::Relaxed), id, a, b);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Frame {
    id: u64,
    key: u64,
    width: u32,
    height: u32,
}

pub(crate) fn frame(width: u32, height: u32) -> Option<Frame> {
    let trace = active()?;
    Some(Frame {
        id: trace.frames.fetch_add(1, Ordering::Relaxed) + 1,
        key: trace.latest_key.load(Ordering::Relaxed),
        width,
        height,
    })
}

pub(crate) fn surface(frame: Option<Frame>, event: &'static str) {
    if let (Some(trace), Some(frame)) = (active(), frame) {
        trace.emit(
            event,
            frame.key,
            frame.id,
            u64::from(frame.width),
            u64::from(frame.height),
        );
    }
}
