# Input Architecture

Status: implementation contract for `feature/refactor-input` (2026-09-20)

This document defines the input model for Scarlet, including direct touch,
indirect touchpads, mice, keyboards, gamepads, system gestures, and popup input.
The [workspace shell](workspace-shell.md) defines the product actions; this
document defines how physical input reaches the owner of each action.

## Boundaries

| Owner | Responsibility | Must not do |
| --- | --- | --- |
| Device driver | Publish physical events, capabilities, axis ranges, direct/indirect property, and synchronization frames | Interpret application or shell gestures |
| Common input core (initially in SWS) | Normalize device reports, maintain per-device contact/button lifetimes, identify sources and seats, and emit atomic frames or cancellation | Hit-test windows or choose actions |
| SWS seat/router | Map direct coordinates to outputs, choose and retain input targets, reserve system gesture origins, manage pointer/keyboard focus and popup grabs | Interpret widget gestures or use focus changes as popup dismissal |
| ScarletUI | Route touch to a widget path and arbitrate tap, scroll, drag, long press, and pinch recognizers | Reconstruct touch from mouse packets |
| Shell/application | Execute semantic actions; shell chooses navigation and menu policy | Reinterpret physical device packets |

The common input core is a reusable library in the SWS process. A separate
daemon would need a synchronized copy of SWS scene and focus state to route
events and provides no current benefit. Physical device identity and trust
remain internal to SWS; clients receive an opaque seat and per-interaction IDs.

## Frame and device contract

The kernel ABI remains `InputEvent { time, type, code, value }` and `SYN_REPORT`
for a complete device report. Each device declares its class and capabilities;
type-B touch declares slot count, X/Y ranges, and directness. Axis resolution or
physical dimensions are optional metadata to add to the device ABI. Gesture
distance thresholds use physical units when available, with an explicit
fallback profile otherwise. Normalizing every device to `0..10000` alone does
not make a distance physically comparable between devices.

The core emits one atomic `InputFrame` per source report:

```rust
struct InputFrame {
    seat_id: SeatId,
    device_instance: DeviceInstanceId,
    frame_no: u64,
    time_ns: u64,
    events: Vec<LogicalInput>,
}

enum TouchPhase { Down, Move, Up, Cancel }
struct ContactChange { id: ContactId, phase: TouchPhase, x: i32, y: i32, /* optional axes */ }
enum LogicalInput {
    Pointer(/* source kind, position/delta, buttons, axis phase */),
    Touch(Vec<ContactChange>),
    Keyboard(/* physical key transition and repeat */),
    Gamepad(/* snapshot and source */),
    Switch(/* posture or lid state */),
    SourceReset,
}
```

`DeviceInstanceId` changes on reconnect; `ContactId` is never reused within an
active stream. The input queue holds whole frames, preserving order within each
device. SWS assigns a seat serial as it dispatches frames. Source timestamps
are preserved for velocity calculations; global sorting by timestamps from
independent or remote devices is not required. Motion may be coalesced only
within the same source, owner, and phase. A queue overflow, `SYN_DROPPED`,
disconnect, seat disable, or output remap must cancel affected active state
before accepting new input; down/up/cancel transitions cannot be dropped
silently.

Gamepads use this same source, seat, frame, and reset contract. The client
device ID is unique per reader instance, even when `/dev/gamepadN` is reused.
A gamepad frame contains one authoritative button/axis/hat snapshot, not a
series of synthetic keyboard packets. The seat routes that snapshot to the focused application
only while it subscribes to raw gamepad input; losing focus, unsubscribing,
disconnecting, or dropping a device frame sends a source-scoped reset to the
previous owner. A separate seat policy may turn the snapshot into navigation
intent (direction, confirm, cancel, home), with repeat owned by the seat and
button layout configured outside the driver. If raw input and navigation are
both exposed to an application, the default client profile suppresses
navigation for that application so a button cannot activate twice. An explicit
request for both paths must handle duplicate semantic actions. Home remains a
reserved system action. Navigation and raw state are derived from the *same* input frame and
must be dispatched in a fixed order; neither path reopens a dropped frame.

Touchscreens and indirect touchpads enter the same core as distinct device
classes. A direct contact is never converted to a mouse event before routing.
Button transitions on a direct-touch device are classified against the whole
contact report, not discarded solely because of the device class. A BTN_LEFT
transition that mirrors an active contact is suppressed; one with no contact
can be an independent pointer click, as with QEMU's shared input dispatcher.
The decision remains source-scoped, and a forwarded press retains its matching
release even if a contact begins in between.
An indirect pad may produce pointer motion, tap-to-click, scroll, pinch, or
swipe centrally; physical clickpad buttons suppress a duplicate synthetic tap.
The classifier remains pending until a movement threshold separates scroll
from pinch/swipe, then keeps that class until end or cancellation. Only the
SWS system policy maps a generic gesture to navigation.

## Seat routing and ownership

SWS maintains independent pointer, keyboard, touch, and scroll-transaction
focus for each seat, plus a gamepad navigation target. A physical or virtual
source belongs to one seat. Pointer
button down retains its implicit target until release; touchpad scroll retains
its target from begin through end/cancel. Each direct contact is hit-tested on
`Down` and stays with that surface through `Up`/`Cancel`, even if it moves
outside. Contacts on different surfaces cannot be combined into one client
gesture. Surface destruction sends cancellation to all owned contacts.

System-owned origins (home-indicator edge and compositor-owned scene controls)
are checked on the first direct `Down`. A claimed stream belongs wholly to the
system and cannot leak a partial client press. A system gesture that may start
anywhere on top of an app would require either buffering its undecided initial
frames before client delivery or a documented downstream-cancellation policy;
it cannot retrospectively guarantee that no app side effect occurred.

SWS recognizes reserved navigation and provides begin/progress/end/cancel with
velocity to the shell. The shell selects the semantic destination and submits
the workspace transaction; SWS owns provisional compositor transforms and
rolls them back on cancel or shell disconnect. Shell widgets, including menus,
receive ordinary native input and use ScarletUI's local gesture arena.

SWS delivers typed, target-local frames with seat, serial, time, contact IDs,
phases, and optional axes. The `sws-protocol` extension is capability-gated and
bounded in size. A client opts into native frames or receives the legacy
`INPUT_EVENT` stream, never both for the same physical contact. The legacy
adapter exposes one primary contact per direct-touch stream and does not
promote a secondary contact after primary release. System gestures are claimed
before either native or compatibility delivery.

## ScarletUI gesture arena

`Event::Touch` is distinct from `Event::Mouse`; touch produces no hover.
Each contact retains its target widget path. Recognizers attached to that path
can be pending, accepted, or rejected. Acceptance cancels losing recognizers
and any visual pressed state they own. A tap activates on a successful `Up`;
scroll or drag winning before then cancels the tap. Contacts in the same
surface may form a pinch through their common widget ancestry. The existing
mouse-only `GestureManager` API is a compatibility adapter, not the new arena.
Both the SWS and winit platform adapters must feed native touch to the same
core event and prevent duplicate host-generated compatibility mouse clicks.
Keyboard and gamepad activation call the same semantic widget action.

## Scroll motion ownership

Direct-touch momentum belongs to the scrollable UI target selected by the
client's gesture arena. SWS keeps the contact's source, target surface, and
timestamp intact, but does not synthesize post-release wheel packets: it cannot
see nested view bounds or decide which widget accepted the drag. ScarletUI uses
the small, `no_std` `scarlet-scroll-motion` crate for velocity estimation and
deceleration. The accepted view still owns its offset, clipping, bounds, and
the choice to pass subsequent movement to an ancestor at a bound.
Non-ScarletUI clients can reuse the same motion crate without depending on the
full UI toolkit.
Shell-owned scene navigation remains a separate compositor animation.

The physical `Touch Up` ends the finger contact. ScarletUI emits a distinct
scroll-momentum sequence after a qualifying release; it stays attached to the
accepted scroll owner, advances with elapsed monotonic frame time, and ends at
the content bound or when velocity decays. A new touch, wheel or key press,
target removal, or window suspension cancels it. A pause before release must
not become a fling. Scroll views receive terminal events on cancellation so
their indicators cannot remain active. This shared controller is UI behavior,
not a driver-specific gesture recognizer.

Indirect touchpad momentum remains a separate decision. The current SWS
touchpad recognizer emits begin/update/end internally, but its client path
turns updates into legacy `MouseWheel` packets and loses source, phase, timing,
and fine displacement. A future scroll IPC message must preserve these before
choosing between client/toolkit-owned momentum and compositor-generated
momentum. If the compositor supplies momentum, clients must be able to identify
it so they do not synthesize a second fling. Discrete mouse wheels should not
acquire direct-touch momentum through this change.

## Menus and popup lifecycle

A menu tap commits exactly one action on release. Pointer hover can switch the
open top-level menu; touch cannot. The state reducer is:

| State and input | Next state |
| --- | --- |
| Closed + activate A | Open(A) |
| Open(A) + activate A | Closed |
| Open(A) + activate B | Open(B) |
| Open(A) + pointer hover B | Open(B) |

Popup creation is attached to the parent StatusBar surface, opening seat, and
input serial. SWS owns its stacking and temporary input grab. Dismissal is an
explicit event with a reason, not inferred from `FocusChanged` or window names.
The touch sequence that opens a popup remains owned by the menu bar through its
terminal `Up`; the new popup receives only subsequent sequences. A menu
session generation rejects stale asynchronous creation results. Input in the
parent menu bar can switch menus; an outside down dismisses and is consumed.

## Migration and verification

1. Introduce frame/source types and device capability metadata. Retain the
   existing pointer and keyboard behavior through a boundary adapter. Move
   gamepad raw snapshots, derived navigation, and reset into the same atomic
   frame before replacing the existing gamepad-specific routing policy.
2. Add native SWS input capability, protocol, client decoding, and ScarletUI
   touch dispatch. Route direct contacts to native clients; keep legacy clients
   on the compatibility path.
3. Move touchpad classification to the common core and deliver phased scroll,
   pinch, and swipe. Wire system gesture ownership and shell intent/transaction
   handling.
4. Add popup role/session semantics and migrate shell menus. Remove focus-name
   heuristics after the new path is exercised.
5. Exercise the same native ScarletUI behavior through its SWS and winit
   adapters, then retire legacy touch emulation for native clients.

Record normalized frames and routing decisions for replay. Required scenarios
include menu A-to-B touch, edge gesture without app press, tap versus scroll,
scroll versus pinch, two contacts on separate surfaces, concurrent devices,
disconnect and `SYN_DROPPED`, gamepad held-button focus transfer and
raw/navigation duplication, popup focus/dismissal, and legacy-client behavior.
QEMU's `virtio-tablet` tests absolute pointer input only. A
`virtio-multitouch-device` plus QMP `input-send-event` with `mtt` slots tests
direct touch; the Scarlet VirtIO driver must first publish its type-B axes,
slot count, and direct property instead of classifying the device by name.

## Current implementation boundary

The first slice on `feature/refactor-input` gives local pointer, keyboard, and
gamepad readers unique source instances and queues complete reports. The
compositor assigns seat serials at dispatch. VirtIO advertises touch from
capabilities. Native direct-touch frames cross the version-10 SWS protocol into
ScarletUI and its winit adapter; legacy clients retain the primary-contact
mouse path. The ScarletUI arena delivers tap, drag, scroll, long press on
release, and pinch, and the menu popup subscribes to native contact frames.
Gamepad snapshots now use instance IDs, reset promptly on unsubscription, and
ordinary ScarletUI windows select navigation without duplicate raw delivery.
The touchpad recognizer already lives in SWS; this slice adds a pending
threshold before it commits two-finger motion to scroll.
Direct-touch scrolls now estimate release velocity, animate through the UI
frame loop, transfer subsequent motion to an ancestor at a bound, and cancel
on a new interaction or window suspension. The shared motion math is in
`scarlet-scroll-motion`; indirect touchpad momentum policy is still open.

The QEMU AArch64 integration run exercised an absolute `virtio-tablet` pointer
and `virtio-multitouch-device` in the same guest: a mouse click in Files, a
direct-touch tap, and a mouse click again all reached their targets. Files also
scrolls by touch, and the console Library shelf scrolls horizontally by touch.
Notepad's File menu remained open after a touch release. These devices do not
require a mode switch in SWS. ScarletUI now gives its built-in buttons, menu
items, toggles, selects, sliders, tabs, split dividers, text controls, sidebar
rows, and window titlebar controls native touch actions. Press state is tracked
per source, so touch cancellation does not discard a simultaneous mouse press.
Drag-capable controls claim only their axis over an ancestor scroll view;
perpendicular motion remains available to scrolling. Touch does not synthesize
pointer hover.
QEMU can dispatch `BTN_LEFT` to its most recently activated button-capable
handler while dispatching absolute cursor position to the tablet. When its
multitouch handler receives a mouse click with no contact, SWS now keeps that
button transition; it still suppresses contact-mirror buttons. A fresh QEMU
boot verified touch launch of Files followed by a mouse click on its Pictures
row without changing QEMU's handler order. Concurrent mouse clicks during an
active QEMU touch contact remain ambiguous if QEMU multiplexes both onto one
`BTN_LEFT` stream; independent physical device streams do not have this issue.
The new momentum slice passes host tests and builds for the AArch64 guest.
A separate QEMU trial using release `scarlet-shell` and `sws` builds confirmed
post-release motion on the console Library shelf: a slow 0.15-screen-width
swipe moved the cards by the contact displacement, while the same fast swipe
advanced farther after `Up`.
SWS workspace-focus restoration now preserves a focused, logically visible
`AlwaysOnTop` overlay instead of forcing focus back to a running app. That
forced restoration had made Control Center close immediately in tablet and
console presentations. In QEMU, Control Center opened above Files in tablet
posture, closed on an outside touch, and reopened; after forcing laptop posture,
it also opened above Terminal and closed through the status control.

The remaining migration work is the per-seat router beyond seat 0, a bounded
input and touch IPC backlog with source-scoped cancellation, physical-unit
gesture thresholds once resolution metadata is available, a client pinch/swipe
wire ABI, and explicit popup session/grab/dismissal messages replacing the
shell's focus-name heuristic. QEMU's virtual multitouch device is opt-in, but
the complete guest menu flow still needs an integration run. None of these
unfinished boundaries should be inferred from a passing compile check.

Scarlet pins the companion ScarletUI branch at commit
`6ef3e3c4da42898e8077b3698f08a95f9f718b8c`. The combined AArch64 build
is validated with that published revision and no local Cargo patch.
