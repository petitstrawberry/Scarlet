//! Low-overhead SWS liveness tracing enabled with `SWS_LOG=trace`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use core::time::Duration;
use std::env;
use std::io::{BufWriter, Write};
use std::println;
use std::thread;
use std::time::Instant;

pub(crate) fn frame_profile_requested() -> bool {
    env::var("SWS_FRAME_PROFILE").is_ok_and(|value| value == "1")
        || env::args().any(|arg| arg == "--profile-frames")
}

/// Fixed-size, file-profile-only counters. A copy belongs to one admitted
/// frame; new input accumulates independently while that frame retires.
#[derive(Clone, Copy, Default)]
pub(crate) struct DamageProfile {
    pub(crate) enabled: bool,
    pub(crate) native_commits: u64,
    pub(crate) native_wire_pixels: u64,
    pub(crate) native_clipped_pixels: u64,
    pub(crate) native_max_rect_pixels: u64,
    pub(crate) native_max_bbox_pixels: u64,
    pub(crate) native_peak_owner: u64,
    pub(crate) native_peak_serial: u64,
    pub(crate) native_last_owner: u64,
    pub(crate) native_last_serial: u64,
    pub(crate) native_peak_extent_pixels: u64,
    pub(crate) native_full_bbox_commits: u64,
    pub(crate) native_projected_pixels: u64,
    pub(crate) content_added_pixels: u64,
    pub(crate) content_peak_pixels: u64,
    pub(crate) content_peak_line: u64,
    pub(crate) other_added_pixels: u64,
    pub(crate) other_peak_pixels: u64,
    pub(crate) other_peak_line: u64,
    pub(crate) full_requests: u64,
    pub(crate) full_line: u64,
    pub(crate) policy_first: u64,
    pub(crate) content_promotions: u64,
    pub(crate) pending_pixels: u64,
    pub(crate) pending_bbox_pixels: u64,
    pub(crate) cursor_pixels: u64,
    pub(crate) main_full: u64,
    pub(crate) gpu_forced: u64,
    pub(crate) invalid_caches: u64,
    pub(crate) material_refreshes: u64,
    pub(crate) requested_pixels: u64,
    pub(crate) logical_pixels: u64,
    pub(crate) aged_pixels: u64,
}

impl DamageProfile {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.enabled);
    }
    pub(crate) fn full(&mut self, line: u32) {
        if !self.enabled {
            return;
        }
        self.full_requests = self.full_requests.saturating_add(1);
        self.full_line = u64::from(line);
    }
    pub(crate) fn native(
        &mut self,
        owner: u32,
        serial: u64,
        extent: (u32, u32),
        wire_pixels: u64,
        rects: &[(u32, u32, u32, u32)],
    ) {
        if !self.enabled {
            return;
        }
        self.native_commits = self.native_commits.saturating_add(1);
        self.native_last_owner = u64::from(owner);
        self.native_last_serial = serial;
        self.native_wire_pixels = self.native_wire_pixels.saturating_add(wire_pixels);
        let mut bounds = (u32::MAX, u32::MAX, 0, 0);
        for &(x, y, w, h) in rects {
            let pixels = u64::from(w) * u64::from(h);
            self.native_clipped_pixels = self.native_clipped_pixels.saturating_add(pixels);
            self.native_max_rect_pixels = self.native_max_rect_pixels.max(pixels);
            bounds = (
                bounds.0.min(x),
                bounds.1.min(y),
                bounds.2.max(x.saturating_add(w)),
                bounds.3.max(y.saturating_add(h)),
            );
        }
        let bbox = u64::from(bounds.2.saturating_sub(bounds.0))
            * u64::from(bounds.3.saturating_sub(bounds.1));
        let extent_pixels = u64::from(extent.0) * u64::from(extent.1);
        if bbox >= self.native_max_bbox_pixels {
            self.native_max_bbox_pixels = bbox;
            self.native_peak_owner = u64::from(owner);
            self.native_peak_serial = serial;
            self.native_peak_extent_pixels = extent_pixels;
        }
        if bbox == extent_pixels {
            self.native_full_bbox_commits = self.native_full_bbox_commits.saturating_add(1);
        }
    }
    pub(crate) fn added(&mut self, rect: (i32, i32, u32, u32), content: bool, line: u32) {
        if !self.enabled {
            return;
        }
        let pixels = u64::from(rect.2) * u64::from(rect.3);
        let (sum, peak, cause) = if content {
            (
                &mut self.content_added_pixels,
                &mut self.content_peak_pixels,
                &mut self.content_peak_line,
            )
        } else {
            (
                &mut self.other_added_pixels,
                &mut self.other_peak_pixels,
                &mut self.other_peak_line,
            )
        };
        *sum = sum.saturating_add(pixels);
        if pixels >= *peak {
            *peak = pixels;
            *cause = u64::from(line);
        }
    }
    fn values(
        &self,
        timings: &crate::sgfx_ir_support::FrameTimings,
    ) -> [u64; DAMAGE_COLUMNS.len()] {
        [
            self.native_commits,
            self.native_wire_pixels,
            self.native_clipped_pixels,
            self.native_max_rect_pixels,
            self.native_max_bbox_pixels,
            self.native_peak_owner,
            self.native_peak_serial,
            self.native_last_owner,
            self.native_last_serial,
            self.native_peak_extent_pixels,
            self.native_full_bbox_commits,
            self.native_projected_pixels,
            self.content_added_pixels,
            self.content_peak_pixels,
            self.content_peak_line,
            self.other_added_pixels,
            self.other_peak_pixels,
            self.other_peak_line,
            self.full_requests,
            self.full_line,
            self.policy_first,
            self.content_promotions,
            self.pending_pixels,
            self.pending_bbox_pixels,
            self.cursor_pixels,
            self.main_full,
            self.gpu_forced,
            self.invalid_caches,
            self.material_refreshes,
            self.requested_pixels,
            self.logical_pixels,
            self.aged_pixels,
            timings.damage_slot,
            timings.damage_initialized,
            timings.damage_history_pixels,
        ]
    }
}

const DAMAGE_COLUMNS: [&str; 35] = [
    "native_commits",
    "native_wire_pixels",
    "native_clipped_pixels",
    "native_max_rect_pixels",
    "native_max_bbox_pixels",
    "native_peak_owner",
    "native_peak_serial",
    "native_last_owner",
    "native_last_serial",
    "native_peak_extent_pixels",
    "native_full_bbox_commits",
    "native_projected_pixels",
    "content_added_pixels",
    "content_peak_pixels",
    "content_peak_line",
    "other_added_pixels",
    "other_peak_pixels",
    "other_peak_line",
    "full_requests",
    "full_line",
    "policy_first",
    "content_promotions",
    "pending_pixels",
    "pending_bbox_pixels",
    "cursor_pixels",
    "main_full",
    "gpu_forced",
    "invalid_caches",
    "material_refreshes",
    "requested_pixels",
    "logical_pixels",
    "aged_pixels",
    "age_slot",
    "age_initialized",
    "age_history_pixels",
];

/// One compact row per completed composition; no per-frame console output.
pub(crate) struct FrameProfile {
    file: BufWriter<std::fs::File>,
    epoch: Instant,
    frame: u64,
}

impl FrameProfile {
    pub(crate) fn open() -> Option<Self> {
        if !frame_profile_requested() {
            return None;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("/tmp/sws-frames-{}.tsv", std::process::id()))
            .ok()?;
        let mut file = BufWriter::with_capacity(32768, file);
        Self::write_header(&mut file).ok()?;
        Some(Self {
            file,
            epoch: Instant::now(),
            frame: 0,
        })
    }

    fn write_header(file: &mut impl Write) -> std::io::Result<()> {
        write!(
            file,
            "frame\tbegin_ns\tend_ns\tprepare_ns\tencode_ns\tsubmit_ns\tpresent_ns\tretire_ns\tencode_submit_ns\tbackend_admit_ns\tgpu_wait_ns\tdisplay_enqueue_ns\timage_reuse_wait_ns\tquads\tdamage_pixels"
        )?;
        for column in DAMAGE_COLUMNS {
            write!(file, "\t{column}")?;
        }
        writeln!(file, "\toutput\tother_outputs_pending_at_admission")
    }

    pub(crate) fn record(
        &mut self,
        stages: [Instant; 6],
        timings: &crate::sgfx_ir_support::FrameTimings,
        quads: usize,
        damage_pixels: u64,
        damage: Option<&DamageProfile>,
        output: usize,
        other_outputs_pending_at_admission: usize,
    ) -> std::io::Result<()> {
        let [start, synced, encoded, submitted, presented, end] = stages;
        self.frame += 1;
        write!(
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
        let damage = damage.copied().unwrap_or_default();
        for value in damage.values(timings) {
            write!(self.file, "\t{value}")?;
        }
        writeln!(self.file, "\t{}\t{}", output, other_outputs_pending_at_admission)?;
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
        env::var("SWS_PROFILE").is_ok_and(|value| value == "1")
            || env::args().any(|arg| arg == "--profile"),
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

#[cfg(test)]
mod damage_profile_tests {
    use super::*;
    #[test]
    fn damage_snapshot_separates_raw_rects_from_bbox_and_new_input() {
        let mut counters = DamageProfile::new(true);
        counters.native(6100, 7, (100, 100), 200, &[(0, 0, 100, 1), (0, 99, 100, 1)]);
        counters.added((10, 20, 5, 6), true, 123);
        counters.added((0, 0, 40, 50), false, 456);
        counters.full(789);
        let admitted = counters;
        counters.reset();
        counters.native(6101, 9, (100, 100), 1, &[(0, 0, 1, 1)]);
        assert_eq!(admitted.native_commits, 1);
        assert_eq!(admitted.native_clipped_pixels, 200);
        assert_eq!(admitted.native_max_rect_pixels, 100);
        assert_eq!(admitted.native_max_bbox_pixels, 10000);
        assert_eq!(admitted.native_full_bbox_commits, 1);
        assert_eq!(admitted.native_peak_owner, 6100);
        assert_eq!(admitted.native_peak_serial, 7);
        assert_eq!(admitted.content_added_pixels, 30);
        assert_eq!(admitted.other_added_pixels, 2000);
        assert_eq!(admitted.other_peak_line, 456);
        assert_eq!(admitted.full_line, 789);
        assert_eq!(counters.native_peak_owner, 6101);
        assert_eq!(counters.full_requests, 0);
        let mut disabled = DamageProfile::default();
        disabled.native(6100, 1, (100, 100), 10000, &[(0, 0, 100, 100)]);
        disabled.added((0, 0, 100, 100), false, 1);
        disabled.full(1);
        assert!(
            disabled
                .values(&Default::default())
                .into_iter()
                .all(|value| value == 0)
        );
    }

    #[test]
    fn frame_file_keeps_original_columns_and_numeric_damage_snapshot_schema() {
        let name = std::env::temp_dir().join(format!(
            "sws-damage-profile-{}-{}.tsv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = std::fs::File::create(&name).unwrap();
        let mut file = BufWriter::new(file);
        FrameProfile::write_header(&mut file).unwrap();
        let now = Instant::now();
        let mut profile = FrameProfile {
            file,
            epoch: now,
            frame: 0,
        };
        let mut damage = DamageProfile::new(true);
        damage.native(6100, 7, (100, 100), 200, &[(1, 1, 10, 20)]);
        damage.logical_pixels = 456;
        let timings = crate::sgfx_ir_support::FrameTimings {
            damage_slot: 2,
            damage_initialized: 1,
            damage_history_pixels: 123,
            ..Default::default()
        };
        profile
            .record([now; 6], &timings, 8, 789, Some(&damage))
            .unwrap();
        profile.file.flush().unwrap();
        let data = std::fs::read_to_string(&name).unwrap();
        drop(profile);
        std::fs::remove_file(name).unwrap();
        let mut lines = data.lines();
        let header: Vec<_> = lines.next().unwrap().split('\t').collect();
        let values: Vec<u64> = lines
            .next()
            .unwrap()
            .split('\t')
            .map(|value| value.parse().unwrap())
            .collect();
        assert_eq!(
            &header[..15],
            &[
                "frame",
                "begin_ns",
                "end_ns",
                "prepare_ns",
                "encode_ns",
                "submit_ns",
                "present_ns",
                "retire_ns",
                "encode_submit_ns",
                "backend_admit_ns",
                "gpu_wait_ns",
                "display_enqueue_ns",
                "image_reuse_wait_ns",
                "quads",
                "damage_pixels"
            ]
        );
        assert_eq!(header.len(), values.len());
        assert_eq!(values[13..15], [8, 789]);
        for (field, expected) in [
            ("native_peak_owner", 6100),
            ("native_peak_serial", 7),
            ("native_clipped_pixels", 200),
            ("logical_pixels", 456),
            ("age_slot", 2),
            ("age_initialized", 1),
            ("age_history_pixels", 123),
        ] {
            let column = header.iter().position(|&value| value == field).unwrap();
            assert_eq!(values[column], expected, "{field}");
        }
        assert!(lines.next().is_none());
    }
}
