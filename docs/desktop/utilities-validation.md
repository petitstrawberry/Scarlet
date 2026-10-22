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
with the available cached compiler. Its Scarlet standard library is older than
the current repository; current-toolchain validation remains separate.

Host Winit CPU rendering was exercised with real controls: Clock mode selection,
timer entry/start/pause/resume/finish/reset, stopwatch, Tab focus and minimum
size; Calendar month/date/Today keyboard navigation and minimum size; System
Logs static preview, filter, priority, details, clear and unavailable-backend
states. The host clock uses real system time with an explicit UTC preview
label. Live logd queries and timer/calendar behavior inside a booted Scarlet
guest remain integration checks.

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
