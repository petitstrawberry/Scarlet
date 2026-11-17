# Multi-Display Design

Status: the kernel exposes connectors through `/dev/displayN`, and SWS drives
connected outputs as an extended desktop. Secondary outputs are placed to the
right of the primary. Each output has its own GPU targets, damage history and
asynchronous display queue; uploads, client imports and GPU receipt ownership
remain shared. Pointer movement, CPU/SGFX damage and backdrop materials use
the output layout. Absolute touch remains mapped to the internal panel.

CoachZ board support lives in `scarlet-project-chromebook`: SC7180 DPU
DMA0/LM1/INTF0, the native DP controller, QMP v3 PHY, DISP_CC and EC Type-C
status observation. The internal DSI path is retained independently.

This establishes the output/composition foundation described by Phases 1–3
below. Per-output shell chrome, independent workspace presentation, fullscreen
ownership and output configuration protocol remain later phases. The legacy
screen-size/workarea APIs still describe the primary output. A secondary
monitor does not yet acquire its own Home/taskbar or independent workspace.

The sections below preserve the design and its pre-Phase-1 audit. References
to single-output behavior in that audit describe the original baseline.

## 1. Current State (verified in code)

Line numbers and names in this section describe the tree **before Phase 1**
(commit `84baf6f21`). Phase 1 moved the SWS fields listed below onto
`OutputSet`; see §7 for what changed.

### Kernel

| Assumption | Where |
|---|---|
| One framebuffer resource per graphics *device*, not per output | `FramebufferResource { source_device_id, .. }` — `kernel/src/device/graphics/manager.rs:41` |
| virtio-gpu stores all 16 `pmodes` but every consumer reads only `pmodes[0]` (other scanouts are zeroed only in the synthetic 1920×1080 fallback when scanout 0 is disabled) | `virtio_gpu.rs:1234-1266`, `:1386`, `:1426` |
| virtio-gpu only binds scanout 0 | `SET_SCANOUT { scanout_id: 0 }` in `set_primary_scanout` (`virtio_gpu.rs:1522-1553`) and the async present path (`:1862-1872`); single `scanout_resource_id` field (`:533`) |
| `GraphicsDevice` methods are implicitly "the primary output" | `get_framebuffer_config`, `present_*`, `get_brightness_percent`, scanout swapchain — `kernel/src/device/graphics/mod.rs:563-822` |
| `DisplayOutput::present(config, fb_paddr)` takes a *shared* framebuffer: it can mirror, but cannot express independent scanouts, modes, or hotplug events | `kernel/src/device/graphics/output.rs` |
| `DisplayOutput` is effectively dead: `get_outputs()` has only the empty default (`mod.rs:820`), no driver overrides it, and `SimpleFbOutput` is never constructed. Its only consumers are `mirror_all` / `mirror_to`, which have no callers. `DisplayConfiguration` and `DisplayMode` are unused | `manager.rs:85`, `:1016`, `:1059`; `output.rs` |
| The disabled Apple Silicon DCPext/DPXBAR driver is written to end in a `DisplayOutput` | `kernel/src/drivers/display/mod.rs` |
| `get_graphics_device()` returns "the first" device | `mod.rs:49` (only a test calls it) |
| `/dev/displayN` takes its index from the `fbN` logical name, which is the lowest free index per registered device | `manager.rs:386-388`, `:813` |
| `DisplayInfo` is a fixed `repr(C)` record with no version/size/reserved field | `display_device.rs:77` |
| Resize for scanout 0 already exists in the kernel: `VIRTIO_GPU_EVENT_DISPLAY` → re-query → replace backing (new `backing_id`) | `virtio_gpu.rs:1463-1477` |
| `/dev/displayN` has no readiness signal: `wait_until_ready` returns `Ready` immediately | `display_device.rs:876-900` |

### Userspace

| Assumption | Where |
|---|---|
| `DisplaySurface::open_primary()` / `DisplayControl::open_primary()` probe `/dev/display0..` and take the first match | `user/lib/framebuffer/src/lib.rs:564`, `:659` |
| SWS owns exactly one `DisplaySurface` and one `screen_width/height` pair (173 lines reference them in `compositor.rs` alone) | `user/std-bin/src/sws/compositor.rs:2403-2415`, opened at `:2758` |
| Output resize is detected by re-reading `var_screen_info` on every compositor loop iteration; it then rewrites all global state and broadcasts `SCREEN_SIZE_CHANGED` | `Compositor::check_display_resize` — `compositor.rs:3554-3610`, called from `:7443` |
| Input clamps to a global screen size | `input::set_screen_size` — `user/std-bin/src/sws/input.rs:407` |
| One fullscreen owner for the whole compositor | `FULLSCREEN_OCCUPIED` — `compositor.rs:13025`, spec `sws-ipc-protocol.md` §SET_FULLSCREEN |
| Frame callbacks are paced by one global `event_counter`, advanced once per successful present | increment `compositor.rs:7774`; `frame_callback_is_ready(.., self.event_counter, ..)` at `:10176` |
| Workspace state is single-output today, but the shell design already specifies one active workspace and one presentation/posture *per output* | `WorkspaceManager` (`workspace.rs:30`); `docs/desktop/workspace-shell.md` §Invariants 3–4, §Output and Posture Model |
| Remote capture already addresses outputs by id (`CreateCapture { output_id }`, `OUTPUT_CHANGED`); screenshot requests output 0 | `user/lib/sws-remote-protocol/src/lib.rs:203-212`, `:327`; `docs/desktop/screenshot.md` |
| `SET_FULLSCREEN` and `SET_WORKAREA` payloads are length-checked exactly (4 and 16 bytes) | `user/lib/sws-protocol/src/lib.rs:2433`, `:2698` |
| GPU composition renders one target sized to the screen | `GpuCompositor::compose_and_present(display, ..)`, `resize_target` — `gpu_compositor.rs:1164` |
| One output scale (`[output] scale_milli`) | `compositor.rs:2415`, `sws/config.rs` |
| wayland-bridge advertises one `wl_output` from SWS's screen size | `user/bin/src/wayland_bridge/output.rs:32` |
| Brightness: `displayctl` addresses "first brightness-capable display" | `user/std-bin/src/displayctl.rs` |

### Compatibility constraints

* **Kernel display ioctls** (`DISPLAY_*`, `/dev/displayN`) are consumed by the
  native `framebuffer` library, which the 1.0 native API contract lists among
  native clients (`docs/release/1.0-native-api-contract.md:25`). This design
  *chooses* to treat the existing commands and records as stable: their
  meaning stays the same, and new behaviour arrives as *new* commands.
* **SWS wire** is an internal matched-set protocol (`docs/release/1.0-scope.md`):
  server, `sws-client`, `sws-client-c`, and the platform adapters update
  together, and optional operations are capability-gated. New messages are
  fine; repurposing `SCREEN_SIZE` is not necessary and should be avoided.
  `scarlet-ui` is pinned to an external revision, so it may hold callers that
  in-tree greps cannot see.

## 2. Target Model

```text
GraphicsDevice (virtio-gpu, simple-fb, DCPext...)
  └── Connector  (scanout 0..N / DP port / internal panel)   stable id, EDID, connected?
        └── Mode list, current mode
        └── Output  ── bound to ──> Scanout backing (per output)
                                     │
                       /dev/displayN │ (one endpoint per enabled output)
                                     ▼
SWS: OutputSet { Output { id, name, rect in global space, scale, mode, DisplaySurface,
                          damage, gpu target, presentation clock } ... }
                                     ▲
             global logical coordinate space (union of output rects)
             windows / cursor / input live here
```

Key decisions:

1. **The unit of presentation is the output, not the device.** A device may
   expose several connectors; each enabled connector gets its own backing,
   its own `/dev/displayN`, and its own present path.
2. **Outputs have stable identities.** `/dev/displayN` numbering stays a
   convenience; durable identity is `(device stable id, connector name)` plus
   EDID-derived vendor/product/serial where available. SWS config and saved
   layouts key on that identity, never on `N`.
3. **SWS composes per output.** Each output is a separate render target with
   its own damage, scale, and frame clock. There is no single giant
   cross-output framebuffer (it breaks with mixed scale/refresh and wastes
   bandwidth on hidden areas).
4. **One global logical coordinate space.** Windows, the cursor, and pointer
   input live in global logical pixels. Each output owns a rectangle in that
   space. Single-output Scarlet is the case where the only rect is
   `(0, 0, w, h)`, so existing coordinates do not move.
5. **"Primary" remains a first-class concept** for compatibility: the output
   that legacy `/dev/fb0`, `open_primary()`, `SCREEN_SIZE`, and unqualified
   fullscreen refer to.

## 3. Kernel Design

### 3.1 Driver interface

Replace the dead `DisplayOutput::present(config, fb_paddr)` shape with a
connector-oriented interface. Keep `GraphicsDevice`'s current methods as the
primary-output facade so existing drivers compile unchanged. When the DCPext
driver is re-enabled, it implements connectors directly instead of
`DisplayOutput`.

Implemented (`kernel/src/device/graphics/output.rs`):

```rust
pub enum ConnectorKind { Unknown, Internal, External, Virtual }

pub struct ConnectorInfo {
    pub index: u32,                    // within the device; 0 = primary-output methods
    pub name: String,                  // "virtual-0", later "dp0", "internal-panel"
    pub kind: ConnectorKind,
    pub connected: bool,
    pub width: u32, pub height: u32,   // current mode, 0 when disconnected
    pub refresh_mhz: u32,              // 0 = unknown
    pub physical_size_mm: Option<(u32, u32)>,
}

pub trait GraphicsDevice: Device {
    // ... existing primary-output methods unchanged ...
    fn connectors(&self) -> Vec<ConnectorInfo> { /* default: one connector from get_framebuffer_config */ }
}
```

Planned, once a second scanout can be driven (EDID summary and mode lists
join `ConnectorInfo` then):

```rust
pub trait GraphicsDevice: Device {
    fn connector_framebuffer(&self, id: ConnectorId) -> Result<(FramebufferConfig, u64), &'static str>;
    fn present_connector_region(&self, id: ConnectorId, region: DisplayRegion) -> Result<(), &'static str>;
    fn present_connector_gpu_resource(&self, id: ConnectorId, resource: GpuDisplayResource,
                                      region: DisplayRegion, options: GpuPresentOptions) -> Result<(), &'static str>;
    fn connector_brightness(&self, id: ConnectorId) -> Result<u8, &'static str>;
    fn set_connector_brightness(&self, id: ConnectorId, percent: u8) -> Result<(), &'static str>;
}
```

Delegation goes **one way only**. The new per-connector methods have default
implementations that accept `ConnectorId(0)` by calling the existing
primary-output methods, and reject other ids. The existing methods stay
required (no default), so a driver can never inherit a cycle.
`simple_framebuffer` and the boot-framebuffer paths become single-connector
devices with no code change. A multi-connector driver (virtio-gpu) overrides
the per-connector methods and implements its primary-output methods as
calls for its primary connector.

### 3.2 virtio-gpu

* `pmodes` is already stored in full. Expose each enabled entry as a connector
  instead of reading only `pmodes[0]`.
* Turn `scanout_resource_id: Option<u32>` into a per-scanout table and pass the
  real `scanout_id` to `SET_SCANOUT` (both the sync and async paths).
  `RESOURCE_FLUSH` is keyed by `resource_id`, so each scanout needs its own
  resource. Flushing that resource updates every scanout bound to it, which is
  also how mirroring could be done for free.
* When `VIRTIO_GPU_F_EDID` is negotiated, issue `GET_EDID` per scanout to fill
  `ConnectorInfo::edid`.
* `VIRTIO_GPU_EVENT_DISPLAY` already triggers a re-query. Extend it to diff the
  per-scanout `enabled` / rect and emit connect, disconnect, and mode-change
  events per connector.
* QEMU test: `-device virtio-gpu-pci,max_outputs=2` (and `virtio-gpu-gl`).

### 3.3 GraphicsManager

* Register a `FramebufferResource`-equivalent per *(device, connector)*,
  renamed `OutputResource`, with an `output_id` (monotonic, never reused while
  the system runs) and the stable identity from §2.
* `/dev/displayN` is created per enabled output. Allocation order is primary
  first, then device/connector order, so `display0` keeps meaning "primary".
* Delete `DisplayConfiguration`, `MmapRegion`, `mirror_all`, `mirror_to`
  (unused). Layout policy belongs in SWS, not the kernel.
* Add a **display hotplug event source**. SWS polls `var_screen_info` every
  loop iteration, and `/dev/displayN` has no readiness signal today. That works
  for one output but cannot discover a *new* `/dev/displayN`. Proposal: make
  `DisplayCharDevice` readable/selectable for "this output's mode or
  connection changed", and add one manager-level node (for example
  `/dev/displayctl`) that is readable for "the output set changed". The
  per-device `display_update_pending` mechanism is the natural producer.
* Removal semantics (proposed, to be validated against the boot-framebuffer
  retirement code in `manager.rs:509-650`): when a connector disappears, its
  `/dev/displayN` is unregistered. Open handles return an `ENODEV`-equivalent
  from every `DISPLAY_*` call, and existing mappings remain valid until they
  are unmapped.

### 3.4 Display ioctl ABI (additive)

Existing `DISPLAY_GET_INFO` (`0x5000`) … `DISPLAY_SET_BRIGHTNESS` (`0x5007`)
keep their meaning on every `/dev/displayN`, now scoped to *that* output.

New commands, each record prefixed by `{ size: u32, version: u32 }` so they
can grow:

| Command | Purpose |
|---|---|
| `DISPLAY_GET_OUTPUT_INFO` (`0x5008`) | **Implemented, version 1, 80 bytes.** Connector index and count, name, kind, connected/primary flags, current mode, refresh (mHz), and physical size; 0 means unknown. `output_id` and the EDID summary are appended in a later version, once stable identity is defined. |
| `DISPLAY_GET_MODES` (`0x5009`) | Mode list (user pointer + count) |
| `DISPLAY_SET_MODE` (`0x500A`) | Request a mode. The backing is replaced and `backing_id` changes, as resize does today |
| `DISPLAY_WAIT_VBLANK` / present-completion timestamp (`0x500B`) | Per-output presentation clock (see §4.4). May be deferred: virtio-gpu has no real vblank |

An enumeration path is needed so SWS does not probe `display0..63`:
`readdir` on `/dev` filtered by `display*` is enough at first, because each node
self-describes via `DISPLAY_GET_OUTPUT_INFO`.

The `framebuffer` crate gains `DisplaySurface::enumerate() -> Vec<OutputInfo>`
and `DisplaySurface::open_output(output_id)`. `open_primary()` is kept and
defined as "the output flagged primary".

## 4. SWS Design

### 4.1 Output object

Gather everything that is per-output today but spread across `Compositor`:

```rust
struct Output {
    id: OutputId,                 // kernel output_id
    identity: OutputIdentity,     // stable key for config
    name: String,
    display: DisplaySurface,
    mode: (u32, u32, u32),        // physical w, h, refresh mHz
    scale_milli: u32,
    logical_rect: Rect,           // in global logical space
    backbuffer: Vec<u8>,          // CPU path
    gpu_target: Option<GpuTarget>,
    pending_damage: Vec<Rect>,    // in output-local physical pixels
    presented_damage: Vec<PresentDamage>,
    presentation_counter: u64,
    fullscreen_owner: Option<WindowId>,
    enabled: bool,
}

struct OutputSet { outputs: Vec<Output>, primary: OutputId, layout: LayoutPolicy }
```

`Compositor { display, screen_width, screen_height, output_scale_milli, backbuffer, pending_damage, presented_damage, .. }`
becomes `Compositor { outputs: OutputSet, .. }`.

Migration rule for the `screen_width/height` uses (173 lines in
`compositor.rs`, more in `window.rs`, `ipc.rs`, `cursor.rs`, `touch.rs`):
classify each one as

* **Global bounds** (cursor clamp, window placement limits, overview layout):
  use `outputs.global_bounds()` or, better, the output under the relevant
  point/window.
* **Per-output** (backbuffer sizing, damage clipping, present): move into
  `Output` methods.
* **"The screen" in a protocol sense** (`SCREEN_SIZE`, fullscreen, maximize,
  shell workarea): resolve to a specific output (the window's output, or the
  primary).

### 4.2 Window ↔ output association

* Every window has a **home output**: the output its workspace is assigned to
  (§8). Maximize, fullscreen, tablet layout, and shell workarea are computed
  against the home output's logical rect / workarea.
* Every window has an **entered set**: the outputs it currently intersects. It
  drives per-output damage, per-output scale hints, and `wl_surface.enter` in
  the Wayland bridge.
* Moving a window across outputs with different scale sends a configure with
  the new preferred scale; the buffer scale is the client's choice, as today.

### 4.3 Composition

* `compose_and_present` runs **per output**: it clips the scene to
  `output.logical_rect`, translates to output-local coordinates, and applies
  `output.scale_milli`. Windows outside the rect are culled before upload.
* Damage is tracked in global logical space, then split per output by
  intersection, so a change on one monitor does not repaint the others.
* GPU path: one `GpuTarget` per output on the same `GpuCompositor` device.
  Texture/upload caches stay shared, since the same window texture is sampled
  by several targets. Outputs on *different* GPU devices are out of scope; the
  `DISPLAY_PRESENT_IMAGE` handle is a capability of one GPU device.
* Cursor is drawn into whichever output(s) its rect intersects. A per-output
  hardware cursor plane (virtio-gpu cursor queue exists) is an optimisation for
  later.

### 4.4 Frame pacing

Before Phase 1, frame callbacks resolved against one global `event_counter`
that advanced once per successful present. Phase 1 moved it to the primary
output's `present_counter`. Under N outputs:

* Each output has its own present counter and last-present timestamp.
* A window's frame callback is released by its **home output's** present,
  as Wayland compositors commonly do. This prevents a 60 Hz and a 144 Hz
  output from fighting over one counter.
* `frame_callback_is_ready` already takes the counter as an argument, so the
  policy is unchanged. The `frame_callback_is_ready` call site passes the
  home output's counter, and `note_frame_submission` records against the same
  counter.

### 4.5 Input

* Pointer motion is clamped to the **union of output rects**, not their
  bounding box, so the cursor cannot enter dead space in an L-shaped layout.
  When the union check fails, project the cursor onto the nearest output edge.
* Absolute devices (touchscreen, tablet) are mapped to **one output**. The
  default is the internal or primary output, configurable by device identity in
  `sws` config. `set_screen_size` becomes `set_output_layout(&[OutputRect])`
  plus a per-device target.
* Keyboard focus is global (one seat). Output does not change focus rules.

### 4.6 Hotplug and mode change

`check_display_resize` (`compositor.rs:3554`) is already the right shape. It
refreshes the mapping, resizes buffers, resizes the GPU target, notifies
capture, broadcasts, and reconfigures windows. Generalise it into:

* `on_output_added(info)`: open the surface, place it with the layout policy,
  create the targets, then broadcast `OUTPUT_ADDED`.
* `on_output_changed(id)`: run today's resize body against that output.
* `on_output_removed(id)`: release its fullscreen owner (`UNSET_FULLSCREEN`
  semantics), unassign its active workspace (the windows stay in it, per §8),
  move the cursor onto a remaining output, then broadcast `OUTPUT_REMOVED`. If
  the primary is removed, promote another output first.

Polling stays as a fallback until the kernel event source (§3.3) exists.

### 4.7 Configuration

```toml
[[output]]
match = { connector = "dp0" }            # or { edid = "DEL:A0B1:SERIAL" }
position = [1920, 0]                      # logical px
scale = 1.5
mode = "2560x1440@60"
primary = false
enabled = true
```

Unmatched outputs are auto-placed to the right of the existing layout at their
preferred mode and an auto scale (from `physical_size_mm`). The existing
`[output] scale` stays and means "default scale for outputs with no matching
entry".

## 5. SWS Wire Protocol (additive)

Keep `GET_SCREEN_SIZE` / `SCREEN_SIZE` / `SCREEN_SIZE_CHANGED` unchanged,
defined as **the primary output's logical size**.

Capability-gated additions:

| Message | Direction | Payload |
|---|---|---|
| `GET_OUTPUTS` | C→S | — |
| `OUTPUTS` | S→C | list of `{output_id, name, logical x,y,w,h, physical w,h, refresh mHz, scale_milli, flags(primary, internal)}` |
| `OUTPUT_ADDED` / `OUTPUT_CHANGED` / `OUTPUT_REMOVED` | S→C broadcast | single record / `output_id` |
| `WINDOW_OUTPUTS` | S→C | `window_id`, entered `output_id` list, home `output_id` |
| `SET_FULLSCREEN_ON_OUTPUT` | C→S | `window_id`, `output_id`. A new message because `SET_FULLSCREEN` is length-checked to exactly 4 bytes. Plain `SET_FULLSCREEN` means "home output" (today the primary) |
| `SET_OUTPUT_WORKAREA` | C→S | `output_id`, x, y, w, h. A new message because `SET_WORKAREA` is length-checked to exactly 16 bytes. Plain `SET_WORKAREA` sets the primary output's workarea |

`FULLSCREEN_OCCUPIED` becomes per output: one fullscreen owner per output.

Clients that never send `GET_OUTPUTS` keep seeing exactly the single-output
world. Shell clients (`scarlet_shell`, `scarlet_desktop`, `soft_keyboard`,
taskbar) need explicit per-output decisions and are listed in §7.

## 6. Peripheral Components

| Component | Decision |
|---|---|
| wayland-bridge | One `wl_output` global per SWS output, plus `xdg_output` logical geometry from `OUTPUTS`. `wl_surface.enter/leave` from `WINDOW_OUTPUTS`. `xdg_toplevel.set_fullscreen(output)` maps to the output-targeted `SET_FULLSCREEN` |
| remote capture (`sws-remote-protocol`, SWS `capture_session`) | Already carries `output_id` and `OUTPUT_CHANGED`. Make `output_id` the kernel/SWS `OutputId` (today it is always 0) and route `capture_session.output_changed` per output |
| screenshot | Built on remote capture and requests output 0. Default to the output under the cursor; a stitched "all outputs" capture is optional later |
| remote desktop (VNC) | Serve one output, selected via the same capture `output_id` |
| displayctl | `displayctl [--output NAME] get|set`. Default is the internal panel, then the primary |
| `/dev/fb0` legacy | Always aliases the primary output; never extended |
| scarlet-ui (external rev) | Audit for `GET_SCREEN_SIZE` / `SCREEN_SIZE_CHANGED` assumptions before Phase 4 ships |

## 7. Phasing

Each phase must leave single-output behaviour bit-identical and keep the
existing tests green.

1. **SWS refactor, N = 1.** Introduce `Output` / `OutputSet` and move display,
   scale, backbuffer, damage, GPU target, present counter, fullscreen owner,
   and the per-output part of `WorkspaceManager` (§8) into it.
   Classify and migrate `screen_width/height` uses. Exit criterion: no direct
   `screen_width` field left on `Compositor`; behaviour unchanged.

   Done:
   * `sws/output.rs`: `OutputRect`, `Output<T>`, `OutputSet<T>` (primary,
     lookup, bounds, point-at, union clamp) with host tests in
     `tests/test-tablet-geometry.sh`.
   * `Compositor` holds `outputs: OutputSet<DisplayTarget>`. The display
     surface, size, scale, bytes per pixel, CPU backbuffer, swapchain
     present history, and present counter (formerly `event_counter`) live on
     the primary output. `screen_width()`, `screen_height()`,
     `output_scale_milli()`, and `present_counter()` are primary-output
     accessors. The literal exit criterion (no `screen_width` field) is met.
     Its intent (each use classified) is not: every use still means "the
     primary output".
   * `WorkspaceManager` groups `active_workspace`, `normal_workspace`, and
     `presentation` into an `OutputWorkspace` (`primary_output`).

   Verified on 2026-11-13 in QEMU (aarch64, HVF):
   * CPU path (`virtio-gpu-pci`): boot, cursor damage, Overview, and VNC
     client resizes (1280×800 → 1024×768 → 1440×900).
   * GPU path (`virtio-gpu-gl-pci`, cocoa GL): boot to the desktop, checked
     by eye.

   Remaining. Each item can finish Phase 1 now or move into Phase 4, where a
   second output makes it testable; not yet decided:
   * Classify each `screen_width()` / `screen_height()` call as global,
     per-output, or primary (§4.1). Today they all resolve to the primary.
   * Per-output `GpuCompositor` targets (§4.3), per-output fullscreen owner
     (currently derived from window flags), and turning `primary_output` into
     a map keyed by `OutputId`.
   * Global `pending_damage`, `full_redraw_needed`, and CPU `backdrops` stay on
     `Compositor`. Damage is global by design (§4.3); backdrops move with
     per-output CPU composition.
   * `input::set_screen_size` and `Cursor::set_position` still clamp to the
     primary rectangle; switch them to `OutputSet::clamp_point`.
2. **Kernel connector model, N = 1.** Add `ConnectorInfo` / per-connector
   methods with defaults, add `OutputResource` and `DISPLAY_GET_OUTPUT_INFO`,
   and delete the dead stubs. No user-visible change.

   Done:
   * `ConnectorInfo` / `ConnectorKind` (`device/graphics/output.rs`) and
     `GraphicsDevice::connectors()`, defaulting to one connector built from
     the framebuffer configuration.
   * virtio-gpu reads `num_scanouts` and lists one `virtual-N` connector per
     scanout. A scanout is connected only while the host reports it enabled
     with a non-empty rectangle.
   * `DISPLAY_GET_OUTPUT_INFO` (`0x5008`) with an 80-byte, fixed-width,
     size/version-prefixed record that is copied page-safely. The `framebuffer`
     crate gains `get_output_info()`, and `displayctl info` prints every
     `/dev/displayN`.
   * `DisplayOutput`, `SimpleFbOutput`, `DisplayMode`, `mirror_all`,
     `mirror_to`, `DisplayConfiguration`, and `MmapRegion` are removed.
   * Kernel tests cover the default connector, the ioctl contract (short,
     long, and null buffers), and virtio-gpu enumeration. Verified in QEMU
     with `virtio-gpu-pci,max_outputs=2`: `displayctl info` reports
     `virtual-0 (0/2) connected`. Sending a resize for head 1 from a second
     VNC server left head 0 unaffected, but whether head 1 became enabled was
     not observable: `/dev/display0` only describes connector 0.
   * The GPU path (`virtio-gpu-gl`) was not re-run after these kernel changes.

   First task of the next step: measure which scanouts QEMU reports as
   enabled with `max_outputs=2` under VNC and cocoa, and confirm that the
   "connected" definition matches.

   Deferred to the next step, where they get callers and can be tested:
   per-connector present and brightness methods, and the
   `FramebufferResource` → `OutputResource` rename. `output_id` is not in the
   ioctl record yet because its meaning (stable identity) is not settled.
3. **virtio-gpu multi-scanout + event source.** Add per-scanout resources,
   `GET_EDID`, and connect/disconnect events. Test with QEMU `max_outputs=2`;
   SWS now discovers connected secondary endpoints and composes each output.
4. **SWS multi-output.** Add enumeration, layout policy, per-output
   composition and pacing, input union clamp, hotplug, and wire additions.
   Update the shell clients and wayland-bridge.
5. **Polish.** Per-output config, mode setting, hardware cursor, screenshot
   and VNC output selection, and per-output brightness in `displayctl`.

Real hardware target after Phase 4: Apple Silicon internal panel + DCPext
external DP (`kernel/src/drivers/display/mod.rs`, currently disabled pending
late/deferred probe). That pairing forces mixed scale *and* mixed refresh, so
the design must not assume uniform outputs.

## 8. Workspaces and Presentation

The workspace model is **already decided** by
[workspace-shell.md](../desktop/workspace-shell.md), and this design follows
it:

* Exactly one workspace is active per connected output, and a workspace is
  assigned to at most one output at a time (Invariants 3–4).
* `OutputExperience` (Auto/Desktop/Tablet) and `OutputPresentation`
  (Home/Workspace/Overview) are per output. An external monitor may run
  Desktop while the internal panel runs Tablet.
* Detaching an output leaves its workspace unassigned, not deleted.

Consequences for SWS:

* `WorkspaceManager`'s `active_workspace`, `normal_workspace`, and
  `presentation` are per-output state, while the ordered workspace list stays
  global. Phase 1 grouped them into one `OutputWorkspace` field
  (`primary_output`). Phase 4 turns it into a map keyed by `OutputId`.
* A window's home output (§4.2) is the output its workspace is assigned to.
  Geometric "largest overlap" is used only to *move* a window to another
  output's workspace when it is dragged across.
* Tablet layout and shell workarea are computed per output.
* On output removal (§4.6), the windows stay in their now-unassigned
  workspace. That workspace is not merged into the primary's.

## 9. Product Decisions

Decided (2026-11-13): every recommendation below was accepted.

1. **Extend or mirror first?**
   *Decision:* extend. workspace-shell.md already treats mirroring as a
   separate feature, and virtio-gpu can mirror cheaply by binding one
   resource to two scanouts, so the kernel `mirror_all` path is unnecessary.
2. **Where does an unassigned workspace go when its output is unplugged?**
   It stays unassigned (by the shell spec), but what does the user see?
   *Decision:* the primary's Overview shows unassigned workspaces as
   cards that can be opened on the current output.
3. **Shell chrome:** taskbar/Home on every output or only on the primary?
   *Decision:* every output in Desktop experience, because each output
   has its own presentation.
4. **Pointer crossing between a Tablet-experience output and a
   Desktop-experience output:** is it allowed, or confined until an explicit
   switch? *Decision:* allow it; touch stays mapped to its own output.

## 10. Not Covered

* Outputs driven by different GPU devices (cross-device import / PRIME-style
  copies).
* HDR / wide-gamut / per-output color management.
* Variable refresh rate.
* `gpu_compositor.rs` internals beyond the `compose_and_present` /
  `resize_target` boundary. The per-output target split in §4.3 should be
  validated against them during Phase 1.
