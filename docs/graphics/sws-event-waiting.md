# SWS application event waiting

ScarletUI's SWS backend uses `Connection::wait_for_window_events` to sleep
until connection activity or its caller's timeout. It does not poll the
compositor at a fixed 1 ms interval or sleep through incoming frame grants.
The compositor's frame-pacing policy is unchanged.

The connection checks its ordinary event queue and **all** window subscriber
mailboxes before sleeping. The ScarletUI runner waits through its first
window, but that window can share a connection with an animating second
window. A grant already routed for the second window must prevent sleep.
No application or buffer-lifecycle event is consumed by the wait operation.

The native poll watches both the server socket and a lazy, coalesced wake
socket pair owned by the shared connection. A concurrent dispatcher can
empty the server socket and queue a window event; it signals the second
socket before releasing the transport mutex. Checking mailboxes and rearming
the notification under the same mutex closes the check-to-sleep race.
The mutex is not held during the kernel wait, so other clones can still send
requests and dispatch their replies. Handles remain owned for the whole wait.

SGFX-only lifecycle mailboxes are deliberately excluded from the queued-window
check: their consumers may wait until the next render to collect releases.
Treating those retained notifications as window readiness would spin the UI
loop. New incoming data of any kind may still wake a wait, so readiness is a
hint to dispatch and recheck, not a promise of a complete window event.

Zero timeout only checks readiness. Longer timeouts are bounded by the poll
ABI's signed nanosecond limit. A terminal connection error wakes waiters and
is returned to the caller; ScarletUI queues its existing transport-failure
quit event.

## Verification

Pure client tests cover queued second-window events, consumed queue prefixes,
and isolation of retained SGFX lifecycle events. The native
`/bin/sws-event-wait-smoke` probe covers real Scarlet sockets and polling,
including 32 concurrent-dispatch races and disconnect handling. It uses a
private protocol peer rather than the desktop's connection.

For performance checks, build a release image with the pinned Scarlet Rust
toolchain and keep the QEMU configuration fixed. Compare visible showcase
scenes individually and distinguish application paint FPS from display
refresh. A temporary 1 ms cap is useful for diagnosis but is not the shipped
implementation. Do not remove SWS frame grants or GPU validation to measure
this wait change.

The 2026-09-06 check used Scarlet Rust `9f9ef5a48648` and a release build of
the focused SGFX image: HVF, 8 vCPUs, 16 GiB shared RAM, `virtio-gpu-gl-pci`,
`cocoa,gl=on,retina=on,full-grab=on`,
`coreaudio,out.fixed-settings=off,out.mixing-engine=on`, USB NCM disabled and
vhost-user-video enabled. Network was disabled
and the disk used snapshot mode. QEMU logged audio-input and vhost vring
warnings; successful video decoding was not part of this check.

Cube and Gears each ran alone for about 30 seconds, with no application
polling override. After the first ten nonzero half-second HUD samples, the
remaining 48 samples averaged 59.865 fps for Cube and 59.935 fps for Gears.
These are averages of application HUD samples, not scanout measurements.
The preceding Cube diagnostic with the original fixed sleep measured about
48.4 fps. The native event-wait probe reported every check passed and exited
with status zero. Multi-window interaction and compositor performance under
other workloads still require their own measurements.
