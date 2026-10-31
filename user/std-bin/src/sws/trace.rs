//! Low-overhead SWS liveness tracing enabled with `SWS_LOG=trace`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use core::time::Duration;
use std::env;
use std::io::{BufWriter, Write};
use std::println;
use std::thread;
use std::time::Instant;

/// One compact row per completed composition; no per-frame console output.
pub(crate) struct FrameProfile {
    file: BufWriter<std::fs::File>,
    epoch: Instant,
    frame: u64,
}

impl FrameProfile {
    pub(crate) fn open() -> Option<Self> {
        if !env::var("SWS_FRAME_PROFILE").is_ok_and(|value| value == "1") {
            return None;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("/tmp/sws-frames-{}.tsv", std::process::id()))
            .ok()?;
        let mut file = BufWriter::with_capacity(32768, file);
        writeln!(file, "frame\tbegin_ns\tend_ns\tprepare_ns\tencode_ns\tsubmit_ns\tpresent_ns\tretire_ns\tencode_submit_ns\tbackend_admit_ns\tgpu_wait_ns\tdisplay_enqueue_ns\timage_reuse_wait_ns\tquads\tdamage_pixels").ok()?;
        Some(Self {
            file,
            epoch: Instant::now(),
            frame: 0,
        })
    }

    pub(crate) fn record(
        &mut self,
        stages: [Instant; 6],
        timings: &crate::sgfx_ir_support::FrameTimings,
        quads: usize,
        damage_pixels: u64,
    ) -> std::io::Result<()> {
        let [start, synced, encoded, submitted, presented, end] = stages;
        self.frame += 1;
        writeln!(
            self.file,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.frame,
            start.duration_since(self.epoch).as_nanos(),
            end.duration_since(self.epoch).as_nanos(),
            synced.duration_since(start).as_nanos(),
            encoded.duration_since(synced).as_nanos(),
            submitted.duration_since(encoded).as_nanos(),
            presented.duration_since(submitted).as_nanos(),
            end.duration_since(presented).as_nanos(),
            timings.encode_submit_ns,
            timings.backend_admit_ns,
            timings.gpu_wait_ns,
            timings.display_enqueue_ns,
            timings.image_reuse_wait_ns,
            quads,
            damage_pixels
        )?;
        if self.frame % 60 == 0 {
            self.file.flush()?;
        }
        Ok(())
    }
}

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
static PROFILE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable per-frame compositor timings only for an explicit profiling run.
pub(crate) fn profile_enabled() -> bool {
    PROFILE_ENABLED.load(Ordering::Relaxed)
}
static COMPOSITOR_STAGE: AtomicU32 = AtomicU32::new(STAGE_STARTING);
static GPU_WINDOW_ID: AtomicU32 = AtomicU32::new(0);
// Diagnostic counts wrap at the native atomic width. Snapshots retain that
// width so wrapping_sub handles a rollover between watchdog samples.
static COMPOSITOR_LOOPS: AtomicUsize = AtomicUsize::new(0);
static COMPOSITOR_PRESENTS: AtomicUsize = AtomicUsize::new(0);
static IPC_CLIENT_LOOPS: AtomicUsize = AtomicUsize::new(0);
static IPC_POLLS: AtomicUsize = AtomicUsize::new(0);
static IPC_POLL_READY: AtomicUsize = AtomicUsize::new(0);
static IPC_SOCKET_READY: AtomicUsize = AtomicUsize::new(0);
static IPC_WAKE_READY: AtomicUsize = AtomicUsize::new(0);
static IPC_POLL_FATAL: AtomicUsize = AtomicUsize::new(0);
static IPC_POLL_SPURIOUS: AtomicUsize = AtomicUsize::new(0);
static IPC_FRAMES: AtomicUsize = AtomicUsize::new(0);
static IPC_FLUSH_PROGRESS: AtomicUsize = AtomicUsize::new(0);
static WAKE_CALLS: AtomicUsize = AtomicUsize::new(0);
static WAKE_COALESCED: AtomicUsize = AtomicUsize::new(0);
static INPUT_LOOPS: AtomicUsize = AtomicUsize::new(0);
static INPUT_EVENTS: AtomicUsize = AtomicUsize::new(0);
static INPUT_EMPTY: AtomicUsize = AtomicUsize::new(0);
static KEYBOARD_LOOPS: AtomicUsize = AtomicUsize::new(0);
static KEYBOARD_EVENTS: AtomicUsize = AtomicUsize::new(0);
static KEYBOARD_SHORT_READS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Default)]
struct Snapshot {
    compositor_loops: usize,
    compositor_presents: usize,
    ipc_client_loops: usize,
    ipc_polls: usize,
    ipc_poll_ready: usize,
    ipc_socket_ready: usize,
    ipc_wake_ready: usize,
    ipc_poll_fatal: usize,
    ipc_poll_spurious: usize,
    ipc_frames: usize,
    ipc_flush_progress: usize,
    wake_calls: usize,
    wake_coalesced: usize,
    input_loops: usize,
    input_events: usize,
    input_empty: usize,
    keyboard_loops: usize,
    keyboard_events: usize,
    keyboard_short_reads: usize,
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
    PROFILE_ENABLED.store(
        env::var("SWS_PROFILE").is_ok_and(|value| value == "1"),
        Ordering::Relaxed,
    );
    let enabled =
        env::var("SWS_LOG").is_ok_and(|value| matches!(value.as_str(), "trace" | "TRACE" | "4"));
    ENABLED.store(enabled, Ordering::Release);
    if !enabled {
        return;
    }

    println!("[SWS_TRACE] watchdog enabled; interval=2s");
    if thread::Builder::new()
        .spawn(|| {
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
        })
        .is_err()
    {
        println!("[SWS_TRACE] failed to start watchdog thread");
    }
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
