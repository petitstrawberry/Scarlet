# Desktop utility validation

Clock keeps the analog clock and appearance window and adds a countdown timer
and stopwatch. Durations use the OS monotonic clock; the wall clock is used only
for clock/calendar dates. The timer distinguishes ready, running, paused and
finished, and continues while a different Clock page is visible. Closing Clock
ends its timer; it is not a background alarm service.

Calendar provides a Gregorian month view, today/selected-day markings and date
navigation. It deliberately has no event storage. Date arithmetic covers years
1–9999, leap centuries, month-end clamping and day/week/year boundaries.

System Logs reads logd with the existing finite Query protocol. It retains at
most 500 records, limits displayed message bytes, follows with bounded queries,
and offers local filtering, pause, refresh, clear display and record details.
Clear display never deletes logs. Host `--preview` is explicitly static sample
data; without it, a host build reports that live logs require Scarlet OS.

## Checks

The focused `tests/desktop-apps/Cargo.toml` compiles the production source files.
Clock has 8 tests, Calendar 7, and System Logs 9, including timer pause/resume,
the full Gregorian 400-year cycle, bounded journal history, daemon restart,
partial protocol reads, cancellation, timeouts and malformed frames. These
passed on the host; all three apps also passed AArch64 Scarlet target checks
with the current Scarlet Rust/standard-library 26d00a8 toolchain after the
shared typography correction. System Logs' 9 tests were rerun on that toolchain.

Host Winit CPU rendering was exercised with real controls: Clock mode selection,
timer entry/start/pause/resume/finish/reset, stopwatch, Tab focus and minimum
size; Calendar month/date/Today keyboard navigation and minimum size; System
Logs static preview, filter, priority, details, clear and unavailable-backend
states. The host clock uses real system time with an explicit UTC preview
label. The minimum-size smoke was repeated after the shared font-size correction;
log summaries use measured ellipsis while details preserve journal line breaks.
Live logd queries and timer/calendar behavior inside a booted Scarlet guest
remain integration checks.

## Audio recorder prerequisite

A recorder needs a real audio capture path before a record button is useful.
Current `kernel/src/device/audio/mod.rs` documents a playback-only subsystem;
`AudioPlaybackDevice` exposes playback operations, `AudioCharDevice::read_byte`
returns `None`, and `can_read` returns `false`.
`kernel/src/drivers/virtio_snd.rs` is explicitly playback-only and chooses
`VIRTIO_SND_D_OUTPUT` streams. Initializing its RX queue does not implement input.
`user/lib/sas-protocol/src/lib.rs` defines output configuration, drain, close and
output controls, with a client-written PCM ring. The SAS client's `read_frames`
is the playback-consumed counter, not captured samples. Kernel/driver capture,
SAS protocol and client support are required before recorder implementation.

## Storage — 2026-10-23

Storage adds a read-only browser for recursive logical file sizes, with ranked
bars, cached folder drilldown, cancellation and partial-scan reporting. Its own
main lives at `user/std-bin/src/storage/main.rs`; the desktop bundle installs
`/applications/storage.app` with a dedicated icon. The recipe and resources live
beside the application, using the SDK's supported script packaging interface.
It counts file lengths, not allocated blocks or free disk space, and offers no
delete operation. Symlinks are listed without following their targets.

Ten model tests cover scan bounds, cancellation, symlinks, logical sparse/hard-link
sizes, stale results and navigation. A native AArch64 `.app` build passed. Actual
Scarlet QEMU operations inspected a 326 KiB/14-file fixture, entered a 192 KiB/3-file
subfolder, returned to its parent and used File → Choose Folder to inspect a
128 KiB/4-file folder. Evidence is in `artifacts/kureha-workbench-20261023/storage-native/`;
final script-package validation is in `artifacts/storage-20261023/`.
