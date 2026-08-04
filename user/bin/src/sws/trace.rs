//! Low-overhead SWS liveness tracing enabled with `SWS_LOG=trace`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use core::time::Duration;
use std::env;
use std::println;
use std::thread;

pub(crate) const STAGE_STARTING: u32 = 0;
pub(crate) const STAGE_PROCESS_EVENTS: u32 = 1;
pub(crate) const STAGE_FRAME_BATCH: u32 = 2;
pub(crate) const STAGE_GPU_COMPOSITE: u32 = 3;
pub(crate) const STAGE_CPU_COMPOSITE: u32 = 4;
pub(crate) const STAGE_PRESENT: u32 = 5;
pub(crate) const STAGE_WAIT_SIGNAL: u32 = 6;
pub(crate) const STAGE_ACTIVE: u32 = 7;
pub(crate) const STAGE_GPU_SYNC_WINDOWS: u32 = 8;
pub(crate) const STAGE_GPU_ENCODE: u32 = 9;
pub(crate) const STAGE_GPU_SUBMIT: u32 = 10;
pub(crate) const STAGE_GPU_PRESENT: u32 = 11;
pub(crate) const STAGE_GPU_COLLECT_RELEASES: u32 = 12;
pub(crate) const STAGE_GPU_NOTIFY_RELEASES: u32 = 13;

static ENABLED: AtomicBool = AtomicBool::new(false);
static COMPOSITOR_STAGE: AtomicU32 = AtomicU32::new(STAGE_STARTING);
static GPU_WINDOW_ID: AtomicU32 = AtomicU32::new(0);
static COMPOSITOR_LOOPS: AtomicU64 = AtomicU64::new(0);
static COMPOSITOR_PRESENTS: AtomicU64 = AtomicU64::new(0);
static IPC_CLIENT_LOOPS: AtomicU64 = AtomicU64::new(0);
static IPC_POLLS: AtomicU64 = AtomicU64::new(0);
static IPC_POLL_READY: AtomicU64 = AtomicU64::new(0);
static IPC_SOCKET_READY: AtomicU64 = AtomicU64::new(0);
static IPC_WAKE_READY: AtomicU64 = AtomicU64::new(0);
static IPC_POLL_FATAL: AtomicU64 = AtomicU64::new(0);
static IPC_POLL_SPURIOUS: AtomicU64 = AtomicU64::new(0);
static IPC_FRAMES: AtomicU64 = AtomicU64::new(0);
static IPC_FLUSH_PROGRESS: AtomicU64 = AtomicU64::new(0);
static WAKE_CALLS: AtomicU64 = AtomicU64::new(0);
static WAKE_COALESCED: AtomicU64 = AtomicU64::new(0);
static INPUT_LOOPS: AtomicU64 = AtomicU64::new(0);
static INPUT_EVENTS: AtomicU64 = AtomicU64::new(0);
static INPUT_EMPTY: AtomicU64 = AtomicU64::new(0);
static KEYBOARD_LOOPS: AtomicU64 = AtomicU64::new(0);
static KEYBOARD_EVENTS: AtomicU64 = AtomicU64::new(0);
static KEYBOARD_SHORT_READS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Default)]
struct Snapshot {
    compositor_loops: u64,
    compositor_presents: u64,
    ipc_client_loops: u64,
    ipc_polls: u64,
    ipc_poll_ready: u64,
    ipc_socket_ready: u64,
    ipc_wake_ready: u64,
    ipc_poll_fatal: u64,
    ipc_poll_spurious: u64,
    ipc_frames: u64,
    ipc_flush_progress: u64,
    wake_calls: u64,
    wake_coalesced: u64,
    input_loops: u64,
    input_events: u64,
    input_empty: u64,
    keyboard_loops: u64,
    keyboard_events: u64,
    keyboard_short_reads: u64,
}

impl Snapshot {
    fn load() -> Self {
        Self {
            compositor_loops: COMPOSITOR_LOOPS.load(Ordering::Relaxed),
            compositor_presents: COMPOSITOR_PRESENTS.load(Ordering::Relaxed),
            ipc_client_loops: IPC_CLIENT_LOOPS.load(Ordering::Relaxed),
            ipc_polls: IPC_POLLS.load(Ordering::Relaxed),
            ipc_poll_ready: IPC_POLL_READY.load(Ordering::Relaxed),
            ipc_socket_ready: IPC_SOCKET_READY.load(Ordering::Relaxed),
            ipc_wake_ready: IPC_WAKE_READY.load(Ordering::Relaxed),
            ipc_poll_fatal: IPC_POLL_FATAL.load(Ordering::Relaxed),
            ipc_poll_spurious: IPC_POLL_SPURIOUS.load(Ordering::Relaxed),
            ipc_frames: IPC_FRAMES.load(Ordering::Relaxed),
            ipc_flush_progress: IPC_FLUSH_PROGRESS.load(Ordering::Relaxed),
            wake_calls: WAKE_CALLS.load(Ordering::Relaxed),
            wake_coalesced: WAKE_COALESCED.load(Ordering::Relaxed),
            input_loops: INPUT_LOOPS.load(Ordering::Relaxed),
            input_events: INPUT_EVENTS.load(Ordering::Relaxed),
            input_empty: INPUT_EMPTY.load(Ordering::Relaxed),
            keyboard_loops: KEYBOARD_LOOPS.load(Ordering::Relaxed),
            keyboard_events: KEYBOARD_EVENTS.load(Ordering::Relaxed),
            keyboard_short_reads: KEYBOARD_SHORT_READS.load(Ordering::Relaxed),
        }
    }
}

fn stage_name(stage: u32) -> &'static str {
    match stage {
        STAGE_PROCESS_EVENTS => "process-events",
        STAGE_FRAME_BATCH => "frame-batch",
        STAGE_GPU_COMPOSITE => "gpu-composite",
        STAGE_CPU_COMPOSITE => "cpu-composite",
        STAGE_PRESENT => "present",
        STAGE_WAIT_SIGNAL => "wait-signal",
        STAGE_ACTIVE => "active",
        STAGE_GPU_SYNC_WINDOWS => "gpu-sync-windows",
        STAGE_GPU_ENCODE => "gpu-encode",
        STAGE_GPU_SUBMIT => "gpu-submit",
        STAGE_GPU_PRESENT => "gpu-present",
        STAGE_GPU_COLLECT_RELEASES => "gpu-collect-releases",
        STAGE_GPU_NOTIFY_RELEASES => "gpu-notify-releases",
        _ => "starting",
    }
}

/// Start the trace watchdog when `SWS_LOG=trace` is selected.
pub(crate) fn start_watchdog() {
    let enabled =
        env::var("SWS_LOG").is_some_and(|value| matches!(value.as_str(), "trace" | "TRACE" | "4"));
    ENABLED.store(enabled, Ordering::Release);
    if !enabled {
        return;
    }

    println!("[SWS_TRACE] watchdog enabled; interval=2s");
    thread::spawn(|| {
        let mut previous = Snapshot::load();
        loop {
            thread::sleep(Duration::from_secs(2));
            let current = Snapshot::load();
            println!(
                "[SWS_TRACE] stage={} gpu_window={} comp(loop={},present={}) ipc(loop={},poll={}/{},socket={},wake={},fatal={},spurious={},frame={},flush={}) wake(call={},coalesced={}) input(loop={},event={},empty={}) keyboard(loop={},event={},short={})",
                stage_name(COMPOSITOR_STAGE.load(Ordering::Acquire)),
                GPU_WINDOW_ID.load(Ordering::Acquire),
                current
                    .compositor_loops
                    .wrapping_sub(previous.compositor_loops),
                current
                    .compositor_presents
                    .wrapping_sub(previous.compositor_presents),
                current
                    .ipc_client_loops
                    .wrapping_sub(previous.ipc_client_loops),
                current.ipc_poll_ready.wrapping_sub(previous.ipc_poll_ready),
                current.ipc_polls.wrapping_sub(previous.ipc_polls),
                current
                    .ipc_socket_ready
                    .wrapping_sub(previous.ipc_socket_ready),
                current.ipc_wake_ready.wrapping_sub(previous.ipc_wake_ready),
                current.ipc_poll_fatal.wrapping_sub(previous.ipc_poll_fatal),
                current
                    .ipc_poll_spurious
                    .wrapping_sub(previous.ipc_poll_spurious),
                current.ipc_frames.wrapping_sub(previous.ipc_frames),
                current
                    .ipc_flush_progress
                    .wrapping_sub(previous.ipc_flush_progress),
                current.wake_calls.wrapping_sub(previous.wake_calls),
                current.wake_coalesced.wrapping_sub(previous.wake_coalesced),
                current.input_loops.wrapping_sub(previous.input_loops),
                current.input_events.wrapping_sub(previous.input_events),
                current.input_empty.wrapping_sub(previous.input_empty),
                current.keyboard_loops.wrapping_sub(previous.keyboard_loops),
                current
                    .keyboard_events
                    .wrapping_sub(previous.keyboard_events),
                current
                    .keyboard_short_reads
                    .wrapping_sub(previous.keyboard_short_reads),
            );
            previous = current;
        }
    });
}

#[inline]
pub(crate) fn set_compositor_stage(stage: u32) {
    if ENABLED.load(Ordering::Relaxed) {
        COMPOSITOR_STAGE.store(stage, Ordering::Release);
    }
}

#[inline]
pub(crate) fn set_gpu_window(window_id: u32) {
    if ENABLED.load(Ordering::Relaxed) {
        GPU_WINDOW_ID.store(window_id, Ordering::Release);
    }
}

#[inline]
pub(crate) fn ipc_poll_result(
    ready: usize,
    socket_revents: u16,
    wake_revents: u16,
    fatal_mask: u16,
) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if socket_revents != 0 {
        IPC_SOCKET_READY.fetch_add(1, Ordering::Relaxed);
    }
    if wake_revents != 0 {
        IPC_WAKE_READY.fetch_add(1, Ordering::Relaxed);
    }
    if ((socket_revents | wake_revents) & fatal_mask) != 0 {
        IPC_POLL_FATAL.fetch_add(1, Ordering::Relaxed);
    }
    if ready > 0 && socket_revents == 0 && wake_revents == 0 {
        IPC_POLL_SPURIOUS.fetch_add(1, Ordering::Relaxed);
    }
}

macro_rules! counter {
    ($name:ident, $counter:ident) => {
        #[inline]
        pub(crate) fn $name() {
            if ENABLED.load(Ordering::Relaxed) {
                $counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    };
}

counter!(compositor_loop, COMPOSITOR_LOOPS);
counter!(compositor_present, COMPOSITOR_PRESENTS);
counter!(ipc_client_loop, IPC_CLIENT_LOOPS);
counter!(ipc_poll, IPC_POLLS);
counter!(ipc_poll_ready, IPC_POLL_READY);
counter!(ipc_frame, IPC_FRAMES);
counter!(ipc_flush_progress, IPC_FLUSH_PROGRESS);
counter!(wake_call, WAKE_CALLS);
counter!(wake_coalesced, WAKE_COALESCED);
counter!(input_loop, INPUT_LOOPS);
counter!(input_event, INPUT_EVENTS);
counter!(input_empty, INPUT_EMPTY);
counter!(keyboard_loop, KEYBOARD_LOOPS);
counter!(keyboard_event, KEYBOARD_EVENTS);
counter!(keyboard_short_read, KEYBOARD_SHORT_READS);
