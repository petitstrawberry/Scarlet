# Scarlet Workspace Shell

Status: workspace model and initial non-animated implementation accepted

This document defines the Scarlet desktop shell, workspace model, and the
tablet-oriented navigation experience. It replaces the earlier focused-window
policy as the product model for tablet mode. `WindowingMode::Focused` remains a
wire-compatible posture signal during migration, but maximizing every normal
window is not conforming workspace behavior.

## Product Model

Scarlet treats a workspace as one resumable, output-sized screen composition.
The composition is independent of the client-side window state used to render
it.

```text
Session Overview
├── Applications
└── Workspaces[]
    └── AppScenes[]
        └── SurfaceTree
            ├── root surface
            ├── transient dialog
            ├── popup
            └── child surface
```

The persistent hierarchy and the navigation hierarchy are deliberately
different:

- A workspace owns app-scene membership and layout state.
- The session-global application catalog is a non-closable shell surface. It is
  not a workspace.
- Workspace Overview is a zoomed-out projection of the session. Its first depth
  owns workspace/window navigation; its second depth owns the application
  catalog. Both depths are drawn as one output-sized shell canvas rather than a
  stack of independently styled bars, cards, and drawer panels.
- A window is an implementation detail. Tablet navigation never exposes
  maximize, restore, stacking order, or arbitrary window geometry as its user
  model.

One workspace has two retained layout projections:

```text
Workspace
├── DesktopLayout: Freeform(AppScene...)
└── TabletLayout: Empty | Single(AppScene) | Split(AppScene, AppScene, ratio)
```

Changing posture selects a projection. It does not destructively rewrite the
other projection.

## Terminology

### Workspace

A stable, ordered, resumable screen composition. On a laptop it behaves as a
traditional virtual desktop. On a tablet it is the card represented by the
system switcher and presents one app scene or one split pair.

### App scene

The user-visible application unit placed in a workspace. Until SWS has a
first-class scene/surface-tree protocol, one top-level normal window is the
scene root and its transient descendants belong to the same scene.

### Home / application catalog

The global application-launch layer owned by `scarlet-shell`. It is the second
depth of the same shell navigation experience, not a standalone launcher
process. `ShellPresentation::Home` denotes this expanded application drawer;
`Super+Space` enters it directly and toggles back to the workspace selected in
the rail.

### Workspace Overview

The outermost navigation view. Each rail card represents a workspace's current
composition. Laptop Overview combines a compact workspace rail, a non-overlap
spread of windows in the selected workspace, and a shallow pull affordance for
the application layer; expanding the application layer is the next depth.
Tablet Overview is a dedicated large-card switcher because a workspace already
represents one `Single` app or one `Split` pair. Tablet Home retains a compact
workspace rail above the catalog. Both layouts use the same presentation and
input state machine.

### Window switcher

A laptop-only, transient switcher for windows in the current workspace, such
as an Alt-Tab surface. It is not the Workspace Overview and does not exist in
the normal tablet navigation path.

## Invariants

The shell and compositor must preserve these invariants:

1. Workspace identifiers are non-zero and unique within a session.
2. At least one workspace always exists, even while Home is presented.
3. Exactly one workspace is active for each connected output.
4. A workspace is assigned to at most one output at a time. Mirroring is a
   separate future feature.
5. A top-level app scene belongs to exactly one workspace. Transient surfaces
   inherit their root scene's workspace.
6. A tablet layout references zero, one, or two distinct member scenes.
7. A split ratio is clamped to the inclusive range 20% through 80%.
8. Workspace membership, order, focus, layout, and presentation changes become
   visible atomically at a compositor frame boundary.
9. Tablet presentation does not set the public maximized or fullscreen window
   states.
10. Leaving tablet presentation restores the retained freeform geometry rather
    than guessing a new desktop placement.
11. Overview/application-catalog shell surfaces never become members of an
    application workspace.
12. A shell restart does not destroy application surfaces or workspace state.
13. Workspace removal is manual by default. Empty workspaces remain ordered,
    addressable destinations until the user explicitly removes them; the
    tablet top-level launch rule may append a workspace but never cleans one up.
14. The Overview `+` card is a visual creation target, not a workspace. It has
    no `WorkspaceId`, is excluded from the authoritative snapshot, and only
    becomes a real workspace after a click, tap, or successful drop.
15. Removing a workspace never destroys an app scene. Every member is migrated
    atomically to a surviving neighbor, or the removal is rejected.
16. The last workspace cannot be removed.
17. An idle shell does not continuously submit frames. Values derived while
    building a scene are written back to reactive state only when the value has
    actually changed, so scene construction cannot invalidate itself.

## Output and Posture Model

Posture and presentation are per output, even though the initial SWS
implementation exposes one output:

```rust
enum DevicePosture {
    Laptop,
    Tablet,
    Unknown,
}

enum OutputExperience {
    Auto,
    Desktop,
    Tablet,
}

enum OutputPresentation {
    Home,
    Workspace(WorkspaceId),
    Overview { focused: WorkspaceId },
}
```

`Auto` derives the experience from device posture. An external monitor may use
Desktop while the internal display uses Tablet. Closing or detaching an output
makes its workspace unassigned; it does not delete that workspace.

The single-output implementation stores one active workspace, its committed
shell-navigation return target, and one presentation. Moving the Overview
selection updates both identifiers, including when the destination is empty;
occupancy is never used to guess where Overview should return. Protocol fields
and transactions must not assume this state is permanently global.

## Workspace Lifecycle

### Session startup

SWS creates workspace `1` before accepting application surfaces. The shell
registers its exclusive system-shell role, reads the compositor snapshot, and
reconciles any persisted names/order without replacing live compositor state.

### Application launch

Workspace lifetime remains manual, with one tablet launch exception:

- Desktop: add the new top-level scene to the active workspace.
- Tablet: if the selected workspace is empty, fill it. Otherwise atomically
  append a new workspace containing the new top-level scene as its `Single`
  composition, select it, and leave the previous workspace intact.
- Explicit split launch: add the new scene to the requested side of the active
  tablet workspace instead of creating another workspace.
- Transient window: join the parent scene's workspace in both experiences.
- Activation of an existing scene: activate its workspace and focus the scene;
  do not create a duplicate workspace.

Parent assignment is currently a separate SWS request from surface creation.
In tablet mode a new normal surface therefore remains unassigned and hidden
until either its parent relationship arrives or it submits its first real
frame. A parented surface inherits its scene; only a confirmed parentless root
can trigger the tablet launch exception. This avoids briefly publishing and
then abandoning a workspace for a dialog.

The launch token will eventually carry a placement intent. Until that field is
available, SWS applies the defaults above and the shell may reconcile the new
scene in its next generation-checked transaction.

### Closing scenes and workspaces

- Closing one side of a split produces `Single` with the surviving scene.
- Closing the last presented scene produces `Empty`.
- Closing or moving the last scene does not remove or reorder its workspace.
- The `+` pseudo-card creates an empty workspace when clicked or tapped.
  Dropping a scene on it creates the workspace and moves the scene in one
  generation change, so observers never see an intermediate empty target.
- A removable card shows an `×` while hovered. In tablet mode the selected card
  keeps a touch-sized `×` visible because hover is unavailable.
- `Super+Delete` removes the selected Overview workspace or the current normal
  workspace. The final workspace is never removable.
- Desktop removal migrates all members to the left neighbor, or to the right
  neighbor when removing the first workspace. Tablet removal uses the same
  neighbor but is accepted only when it can preserve a valid composition: an
  empty target accepts the source, and `Single` plus `Single` becomes `Split`.
  More complex tablet merges are rejected.
- `/etc/sws/config.toml` may opt into cleanup with
  `[workspaces] auto_remove_empty = true`. The default is `false`. Automatic
  cleanup removes only empty workspaces that are neither the active selection
  nor the normal return workspace; it never reorders survivors.

### Desktop-to-tablet conversion

If a retained tablet layout exists and all of its scenes are still members, SWS
uses it unchanged.

For a workspace without a valid retained tablet layout:

1. Prefer the focused top-level scene.
2. Otherwise prefer the most recently focused member.
3. Otherwise prefer the newest presentable member.
4. Preserve a recognized desktop split/snapped partner when one exists.
5. Otherwise create `Single` from the preferred scene.

Additional desktop members must not become unreachable when posture changes.
Each member that is not part of the retained `Single` or `Split` composition is
materialized as another ordered tablet projection while tablet experience is
active. These projection slots are derived from the retained desktop snapshot,
not user-created workspaces: leaving tablet restores that snapshot exactly.
Every scene remains live and keeps its freeform geometry; it is never maximized
or discarded. A normal tablet launch fills the explicitly selected workspace
when it is empty; otherwise it appends a separate tablet workspace as described
above.

### Tablet-to-desktop conversion

SWS restores the saved freeform geometry and visibility for every member, then
focuses the scene most recently active in the tablet layout. Tablet split
geometry never overwrites the freeform snapshot.

## Layout Semantics

### Responsive Overview layouts

Scarlet has one navigation state machine and two responsive information
layouts. Device posture changes layout, density, and hit-target metrics; it
does not change the meaning of Home, Overview, workspace selection, or their
shortcuts.

The diagrams below show the complete output. During normal workspace use the
StatusBar retains its ordinary material. During Overview and Home its
background becomes transparent, while the existing Home control, system menu,
status items, and clock keep their original component geometry over the shared
canvas. Application context and application menus are hidden because no
application owns the navigation surface.

StatusBar labels and icons use a light foreground over the navigation canvas.
Their existing MenuItem hit boxes and icon metrics remain unchanged; only the
interaction surfaces switch to translucent black (24% hover, 36% active) so a
light highlight cannot erase the light foreground.

#### Pattern A: laptop

A laptop normally has several freeform windows in one workspace. Its first
Overview depth therefore follows the Mission Control / GNOME window-overview
model: an ordered workspace rail and the selected workspace's non-overlapping
window spread are visible at the same time.

```text
┌────────────── one Overview/Home canvas ───────────────────┐
│ [Home 1/3] [Scarlet]                       [status][clock] │
│      ╭── WS 1 ──╮ ╭── WS 2 ──╮ ╭── WS 3 ──╮ ╭─ + ─╮    │
│      │ snapshot │ │ snapshot │ │ snapshot │ │ add │    │
│      ╰──────────╯ ╰──────────╯ ╰──────────╯ ╰─────╯    │
│                                                          │
│       ╭──────── window A ───────╮  ╭── window B ──╮      │
│       │ retained live buffer    │  │ live buffer  │      │
│       ╰─────────────────────────╯  ╰───────────────╯      │
│                  ╭──────── window C ────────╮             │
│                  ╰──────────────────────────╯             │
│                         ━━━━                             │
└──────────────────────────────────────────────────────────┘
```

- The laptop workspace rail uses approximately 10% of the work area, clamped
  to `96..=112` logical pixels. Its viewport spans the full output width and
  its fixed-pitch card content scrolls horizontally without wrapping. When
  content overflows, at most one neighboring card remains clipped at each
  available viewport edge; `+` is the final selectable pseudo-card in the same
  scroll content.
- Rail cards, the selected-workspace stage, and the application layer share one
  cool-slate backdrop. The reference tint is `#4d5769` at 78% opacity: the
  wallpaper remains texture rather than determining the navigation surface's
  color or contrast. The rail has no island, panel, divider, or second
  background fill. Top-level windows are packed into non-overlapping cells
  while preserving their aspect ratios. Small windows are never enlarged. The
  packing is an Overview projection only and never mutates client-visible
  freeform geometry.
- Clicking a spread window activates that scene and leaves Overview. Dragging
  one to a rail card moves it to that workspace. In freeform mode, the drop
  point is inverse-projected into the destination work area and becomes the
  window's new position. While a spread window is dragged upward, its visual
  rectangle interpolates continuously toward its rail-thumbnail size so the
  destination composition remains visible. The grabbed point stays under the
  pointer and transient descendants scale with the root.
- The visible pull control is one centered pill, never a pill plus chevron and
  never an application-grid icon. Its full-width transparent row is the hit
  target; there is no visible bottom bar, dock, or floating sheet.

Activating the pull row or `Super+Space` from the first depth replaces
the selected-workspace stage with the application catalog on the same canvas.
The workspace rail remains fixed. Spread windows are neither visible nor
interactive in `Home:Apps`.

```text
┌────────────── one Overview/Home canvas ───────────────────┐
│ [Home 1/3] [Scarlet]                       [status][clock] │
│      ╭── WS 1 ──╮ ╭── WS 2 ──╮ ╭── WS 3 ──╮ ╭─ + ─╮    │
│      │ snapshot │ │ snapshot │ │ snapshot │ │ add │    │
│      ╰──────────╯ ╰──────────╯ ╰──────────╯ ╰─────╯    │
│                         ━━━━                             │
│ Applications              [ Search ]              count │
│                                                          │
│              icon grid / search-result list              │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

#### Pattern B: tablet

A tablet workspace already exposes its entire `Single` or `Split`
composition, so Overview treats that composition itself as the switchable
unit. The first depth is a dedicated large-card switcher; the application
catalog is not visible there.

```text
┌────────────── one Overview/Home canvas ───────────────────┐
│ [Home 1/3] [Scarlet]                       [status][clock] │
│  ╭────────── WS 1 ──────────╮ ╭──────── WS 2 ────────╮  │
│  │ complete Single/Split    │ │ complete composition │  │
│  │ live composition         │ │ or final + target    │  │
│  ╰──────────────────────────╯ ╰───────────────────────╯  │
│                                                          │
│                         ━━━━                             │
└──────────────────────────────────────────────────────────┘
```

Landscape shows at most two complete large cards. Portrait centers one large
complete card. Other workspaces remain reachable through horizontal movement
and a position indicator; cropped neighbor slivers are never the only evidence
that another workspace exists.

Home uses the same full-output canvas, but makes application launch dominant
and reduces workspace navigation to a compact rail. The rail remains present
because changing the launch destination and explicitly adding a workspace are
valid Home actions.

```text
┌────────────── one Overview/Home canvas ───────────────────┐
│ [Home 1/3] [Scarlet]                       [status][clock] │
│   ╭─ WS 1 ─╮ ╭─ WS 2 ─╮ ╭─ WS 3 ─╮            ╭─ + ─╮  │
│   │snapshot│ │snapshot│ │snapshot│            │ add │  │
│   ╰────────╯ ╰────────╯ ╰────────╯            ╰─────╯  │
│                         ━━━━                             │
│ Applications              [ Search ]              count │
│                                                          │
│              large touch-oriented icon grid              │
│                                                          │
│                         ━━━━                             │
└──────────────────────────────────────────────────────────┘
```

- Overview gives the large workspace cards the complete area between the
  transparent StatusBar and bottom gesture region. It has no drawer, search
  field, or application icons.
- Home gives the catalog the remaining area below its compact complete-card
  rail. `Super+Left` / `Super+Right` traverses real cards and then the final `+`
  pseudo-card without closing Home. `Super+Enter` activates the selected real
  card or confirms creation at `+`; plain `Enter` remains application launch.
- The workspace-card and application regions use the same shared backdrop.
  Cards remain distinct through their own rounded backplates; there is no
  enclosing panel.
- A thin home indicator is visible by default in every tablet presentation.
  Its visual remains small while its reserved system-gesture hit region is
  touch-sized and overlays the safe-area edge instead of shrinking app layout.
  Fullscreen dimming or hiding is a later policy, not the default.
- Direct-touch targets are at least 44 logical pixels. The selected workspace's
  remove control stays visible because touch has no hover state.
- There is no tablet-only Home button behavior. The existing Overview control,
  `Super`, `Super+Space`, keyboard selection, DnD, and Escape have the same
  semantic transitions as on laptop.

#### Shared projection and visual rules

- Rails center every card when their content fits. When content overflows, the
  full-width horizontal viewport shows complete cards plus at most one
  substantial clipped neighbor at each available edge. The clipped card is an
  intentional continuation affordance, not a narrow accidental sliver.
  Tablet Overview uses the same scroll model with larger cards and fewer
  complete cards in its viewport.
- Every card uses a 14-pixel corner radius for its backplate, live-content
  clip, and hit test. Application pixels cannot escape the rounded projection.
- Workspace cards and spread windows use a compact, soft elevation shadow.
  Opacity falls off monotonically through at least six low-alpha steps, corner
  radii follow the projected surface, and the visible spread remains within
  `6..=16` logical pixels. Two opaque nested rectangles or hard halo bands are
  not conforming shadows. A window whose retained surface already includes
  client-rendered shadow outsets keeps that shadow under projection and does
  not receive a second compositor shadow.
- Selection uses live-content opacity and a neutral translucent backplate: a
  faint white veil for the selected card and a faint black veil for inactive
  cards. Cards never use an opaque tinted fill or stacked high-contrast
  outlines.
- Region boxes in the diagrams describe layout bounds, not visible strokes.
  There is no enclosing rail or drawer surface; spacing and the cards'
  individual backplates express structure without double outlines.
- `+` participates in rail scrolling and keyboard selection but has no
  `WorkspaceId`. Selection alone has no side effect; Enter, click, tap, or a
  successful drop is required to create a workspace.
- One client surface may need both a rail thumbnail and a laptop spread
  instance. SWS therefore creates independent read-only presentation instances
  that reference the same retained buffer. It does not create fake client
  windows and does not require a synchronous screenshot round-trip.
- Each presentation instance owns its transform, rounded clip, opacity, and hit
  role. Only the spread instance accepts window clicks/drags; rail instances
  defer input to the workspace card.
- Search belongs to the application header. The unfiltered icon grid has at
  most six columns and reduces that count when the available width cannot keep
  the minimum cell size. Ordinary app icons have transparent cells. Hover uses
  a light translucent surface; keyboard selection uses a stronger light
  surface. Neither state adds an outline.
- The application layer is persistent and is never destroyed and recreated
  between shell depths. Laptop Overview hides its content except for the
  full-width transparent pull row; tablet Overview may leave the configured
  catalog portion exposed. It is not styled as a floating sheet.
- In the initial non-animated implementation, the application layer changes
  depth in one committed frame. Once expanded, the covered window-spread
  instances are hidden and removed from hit testing rather than merely painted
  underneath.
- The Overview/Home surface is assigned its final full-output geometry while
  hidden. Switching Overview depth changes only internal presentation
  instances, so no create-then-move or create-then-resize frame is exposed.
- While Overview or Home is active, compositor input routing consumes pointer
  events over projected application surfaces. Hover, click, and wheel events
  are not forwarded using transformed coordinates to ordinary clients.

### Desktop freeform

SWS retains complete surface geometry, z-order, minimized state, and focus
history for the workspace. Only the active workspace's application surfaces
are eligible for normal composition and input. System shell and overlay roles
are independent of workspace visibility.

### Tablet single

The scene root receives a configure matching the output work area. The public
window state remains normal. Decorations may be suppressed through a future
server-decoration contract; the compositor must not infer this by pretending
the window is fullscreen.

### Tablet split

Version one supports a horizontal split with two scene roots:

```text
available_width = workarea.width - divider_width
first_width     = available_width * ratio_milli / 1000
second_width    = available_width - first_width
```

The divider is compositor input chrome controlled by the shell. Dragging it
updates an interactive preview; release commits a generation-checked layout
transaction. The ratio is stored in milli-units and clamped to `200..=800`.

The data model uses an axis field so a vertical split can be added without a
wire-format break, although version one implements the horizontal axis only.

### Compatibility scenes

Fixed-size or bounded legacy surfaces must not silently float as ordinary
windows in tablet presentation. SWS centers the scene on a system-provided
backplate inside the assigned single/split slot. The workspace card still
represents the scene composition, not a maximized window.

## Navigation

Laptop and tablet share three semantic states. Only their Overview layout is
responsive:

```text
Workspace ── Super ──> Overview:Windows ── drawer affordance / Laptop Space ──> Home:Apps
Workspace <─ Escape ── Overview:Windows <─ Escape / drawer affordance ── Home:Apps

Super from either shell depth ──────────────────────────────> Workspace
Super+Space: Workspace or Overview ─────────────────────────> Home:Apps
Super+Space: Home:Apps ─────────────────────────────────────> Workspace
```

- `Workspace` is the normal application destination.
- `Overview:Windows` is the first Overview depth. Laptop renders the rail,
  selected-workspace window spread, and pill-only pull row. Tablet uses the
  available work area for its dedicated large-card workspace switcher.
- `Home:Apps` is the application-focused depth. Laptop replaces the window
  spread with the catalog. Tablet expands the catalog and collapses cards into
  a rail.
- The Overview button and modifier-only `Super` toggle the complete shell
  experience: from `Workspace` they enter `Overview:Windows`; from either shell
  depth they enter the workspace currently selected in the rail.
- `Super+Space` is the direct application toggle: from `Workspace` or
  `Overview:Windows` it enters `Home:Apps`; from `Home:Apps` it restores the
  workspace currently selected in the rail.
- On laptop, unmodified `Space` moves from `Overview:Windows` to `Home:Apps`.
  Tablet reserves this key for future input policy and does not perform this
  transition. Space keeps its ordinary text-input meaning once Home is open.
- Activating or dragging the pull affordance changes `Overview:Windows` to
  `Home:Apps` and collapses it back without leaving shell navigation. The
  affordance is a pill-only full-width row on laptop; tablet uses the persistent
  bottom home indicator and its upward gesture path.
- Escape first clears a non-empty search. With an empty query it moves one depth
  outward (`Home:Apps` to `Overview:Windows`, then to `Workspace`).

Required gestures:

| Origin | Gesture | Destination/action |
|---|---|---|
| Workspace | home-indicator horizontal swipe | adjacent workspace |
| Workspace | home-indicator swipe up and hold at switcher detent | `Overview:Windows` |
| Workspace | fast/full home-indicator swipe up | `Home:Apps` |
| Overview | upward swipe / raise application layer | `Home:Apps` |
| Overview | downward swipe / cancel shell navigation | selected workspace |
| Home | downward swipe / lower application layer | `Overview:Windows` |
| Overview | tap workspace card | selected workspace |
| Overview | tap an existing empty card | enter that empty workspace |
| Overview | tap `+` pseudo-card | create and select an empty workspace |
| Overview | horizontal scroll or drag | move through ordered workspaces |
| Overview | drag a live scene to an existing card | move the scene; preserve the empty source |
| Overview | drag a live scene to `+` | atomically create a workspace and move the scene |
| Overview | tap a visible `×` | remove the workspace with safe neighbor migration |
| Split | drag divider | interactive split ratio |

Direct-touch navigation contacts are claimed by the compositor before legacy
pointer emulation. Once claimed, their complete contact stream is consumed by
the system gesture arena and must not leak partial press/move/release events to
an application.

Pointer and keyboard equivalents are required for development and laptop use:

- Tap and release `Super`: toggle Workspace Overview; when already open, return
  to the workspace currently selected in the rail. Pressing another key while
  `Super` is held cancels the tap, so `Super` chords do not also toggle
  Overview.
- Shell navigation chords are loaded from `/etc/sws/config.toml` under
  `[keybindings]`: `overview_toggle`, `workspace_left`, `workspace_right`,
  `move_window_left`, `move_window_right`, `home`, and `overview_activate`.
  Workspace lifecycle adds `add_workspace` and `remove_workspace`.
  Bindings match the exact modifier set, so `Super+Shift+Left` and
  `Super+Left` remain distinct actions; changes apply after restarting SWS.
- `Super+Left` / `Super+Right`: adjacent workspace without wrapping. In
  `Workspace` it stops at the first and last real workspace. In
  `Overview:Windows` and `Home:Apps`, moving once past the final real workspace
  selects the final `+` pseudo-card when creation is available; the current
  shell depth remains open.
- `Super+Shift+Left` / `Super+Shift+Right`: move the focused window to the
  adjacent existing workspace and follow it. At either end the action is a
  no-op; it never creates a workspace. The vacated workspace remains even when
  empty.
- `Super+Shift+N`: explicitly append and select an empty workspace.
- `Super+Delete`: explicitly remove the selected/current workspace when its
  members can be migrated safely. It is a no-op for the final workspace or an
  unsafe tablet merge.
- `Super+Space`: toggle the application drawer directly as described above.
- In Overview, ordinary `Left` / `Right` and `Super+Left` / `Super+Right` move
  the selected workspace. The selection is also the committed destination for
  `Escape`, modifier-only `Super`, and the closing half of the `Super+Space`
  toggle; dismissal never jumps back to the workspace selected on entry.
  Moving right once past the final real workspace selects `+`; it never creates
  immediately. `Enter` or `Super+Enter` confirms creation, while Left returns
  to the final real workspace. While `Overview:Windows` is active, printable
  text and application-grid cursor navigation are not routed to the hidden
  drawer.
- In `Home:Apps`, search starts unfocused; typing a printable character focuses
  it. With an empty query, ordinary arrow keys move freely through the
  application icon grid. With a non-empty query, results switch to a list,
  `Up` / `Down` move its selection, and `Left` / `Right` remain text-editing
  keys. `Super+Left` / `Super+Right` traverse the compact workspace rail,
  including its final `+`, without closing Home. `Super+Enter` opens a real
  selected workspace or creates the selected `+`; plain `Enter` launches the
  selected application. `Escape` first clears search, then returns one shell
  depth at a time.
- Alt-Tab: local window switcher, when implemented.

## StatusBar

`scarlet-shell` owns one top StatusBar. The protocol window-role name may remain
`Taskbar` for compatibility, but the product and source-level component name is
StatusBar.

The leading tablet controls are deliberately not collapsed into a single
generic menu:

```text
[ grid 1/2 ] [ App ] [ File ] [ Edit ] [...]            [ status ] [ clock ]
```

- The first button replaces the former Scarlet launcher button. From Home or a
  workspace it opens Overview; pressing it again in Overview returns to the
  workspace currently selected in the rail.
- The button has a fixed 64-pixel width and shows only the grid glyph and
  current/total workspace position. It never expands to fit the words `Home`
  or `Overview`.
- The active application's top-level menus remain separate, horizontally
  ordered touch targets in tablet mode. Tablet presentation increases target
  height and horizontal padding; it does not hide `File`, `Edit`, or other
  application commands behind one permanent "App Menu" button. This applies
  in the normal Workspace presentation; Overview and Home hide application
  context while retaining the Scarlet system menu.
- When width is constrained, optional right-side status labels become compact
  before application menus are removed. Only top-level items that still do not
  fit may move into a trailing, explicit overflow item; all menu commands must
  remain reachable.
- Desktop uses the same item order with denser pointer-oriented metrics.

## Animation Contract

Animations are intentionally disabled during the initial workspace/navigation
implementation. Every state change and input path must first work as an atomic
cut. The tokens below are the final integration stage and must not be enabled
piecemeal while posture conversion, hit testing, and Overview selection remain
under development.

The shell chooses semantic targets and an animation specification. SWS samples
the surfaces and executes transforms on the compositor frame clock.

Initial animation tokens:

| Transition | Duration | Curve |
|---|---:|---|
| Workspace to Home | 240 ms | cubic ease-out |
| Workspace to Overview | 260 ms | cubic ease-out |
| Overview to Workspace | 220 ms | cubic ease-out |
| Adjacent workspace | 280 ms | critically damped spring |
| Split divider settle | 180 ms | critically damped spring |

Interactive gestures drive normalized progress directly. On release, SWS
continues from the current progress and velocity; it does not restart a canned
animation from zero.

During resize and animation SWS retains the last committed client buffer and
scales it into the current visual rectangle. A client configure and replacement
buffer may arrive later. Missing a frame must not produce a blank workspace.

Animations affect compositor presentation transforms only. They do not mutate
client-visible desktop geometry, maximize state, or workspace membership.

## Process and Responsibility Boundary

Scarlet keeps the compositor and shell in separate processes while making the
shell user experience one coherent service.

### `sws`

SWS owns:

- client surface and buffer lifecycle;
- the authoritative live workspace snapshot needed for composition;
- surface-tree and transient relationships;
- frame scheduling and retained buffers;
- presentation transforms, clipping, opacity, and hit testing;
- rounded live-card backplates and the matching geometric clip;
- direct-touch system gesture arbitration;
- atomic workspace transaction application;
- split configure geometry;
- crash-safe preservation of workspaces when the shell disconnects.

SWS does not decide workspace names, persistence policy, Home content,
app-grid order, or Control Center policy. It realizes the minimal shared card
geometry needed to clip live actors; richer shell chrome remains shell policy.

### `scarlet-shell`

One process owns:

- the Overview window layer and application-catalog layer;
- search, drawer, app cells, and optional workspace-card chrome;
- system bars, status surfaces, Control Center, and wallpaper;
- workspace order, names, and user-driven membership changes;
- launch placement intent and split operations;
- transition selection and accessibility alternatives;
- session persistence and restore reconciliation.

Home owns the primary application catalog and launch path. A command palette is
an optional desktop accelerator, not a tablet destination or a permanent
StatusBar menu. The standalone legacy launcher should be retired once its
search/command behavior is available inside the shell.

### Session supervisor and services

`scarlet-desktop` remains a small supervisor. `desktop-settings`, stemd's app
catalog/launch service, audio/network services, and input methods remain
separate processes. They are not part of rendering or navigation state.

## Shell Control Protocol

The shell uses an exclusive role on the SWS connection. Registration succeeds
for only one live connection. Workspace mutation from an unregistered
connection is rejected.

The current Scarlet socket API does not yet expose a strong peer-credential or
launch-capability check, so exclusive registration is a migration boundary, not
the final security boundary. A production multi-user system must pass a shell
capability from the session supervisor or validate peer credentials. This
limitation must not be hidden by advertising ordinary app clients as trusted.

The compositor publishes a monotonically increasing workspace generation.
The shell submits complete, generation-checked transactions:

```rust
struct WorkspaceTransaction {
    base_generation: u32,
    active_workspace: WorkspaceId,
    presentation: ShellPresentation,
    workspaces: Vec<WorkspaceSnapshot>,
    transition: TransitionSpec,
}
```

SWS validates the complete proposed state, applies it atomically, increments
the generation, and broadcasts the resulting snapshot. A stale base generation
is rejected without partial effects. The shell must re-read, reconcile, and
retry.

SWS may independently increment the generation for window creation,
destruction, parent changes, output changes, or posture-derived layout repair.

## Failure and Recovery

- Shell disconnect: retain the current workspace and app surfaces; finish or
  cancel active compositor animations safely; allow a new shell connection to
  register.
- Shell restart: query the live snapshot before applying persisted metadata.
- App crash: remove its scene, repair `Single`/`Split`, and keep the workspace
  valid.
- SWS restart: clients reconnect under the existing toolkit policy; persisted
  workspace metadata is advisory until live scenes return.
- Stale transaction: reject with a typed protocol error and no visual change.
- Invalid split/member reference: reject the complete transaction.
- Unknown posture: preserve the current experience until an explicit policy or
  reliable posture becomes available.

## Persistence

The shell persists only stable policy data:

- workspace order, names, and identifiers;
- app-scene placement hints based on stable app/scene identity;
- tablet split composition and ratio;
- per-output last active workspace;
- last explicit `OutputExperience` override.

Raw `WindowId`, buffer identity, in-progress animation, pointer capture, and
transient popup state are never persisted.

## Implementation Stages

The two responsive layout patterns above are approved. Stages 1 through 4 are
implemented incrementally; animation remains intentionally deferred.

### Stage 1: authoritative workspace foundation

- Add typed workspace snapshots and transactions to `sws-protocol` and
  `sws-client`.
- Add exclusive shell registration.
- Assign top-level normal windows to workspaces.
- Preserve one default workspace and generation across shell restarts.
- Replace focused-mode maximize with tablet `Single`/`Split` presentation
  geometry and retained freeform restoration.

### Stage 2: coherent shell

- Start `scarlet-shell` as the one visual shell process.
- Move Home, Overview, wallpaper, bars, and Control Center into that process.
- Remove the launcher from the StatusBar and keep Home as the primary launch
  surface. Search now lives in the shell and the standalone launcher is not a
  built or supervised desktop component.
- Display live retained workspace cards and provide pointer/keyboard actions.

### Stage 3: responsive Overview projections

- Generalize one compositor surface into multiple read-only presentation
  instances backed by the same retained buffer.
- Implement the laptop workspace rail, selected-workspace non-overlap spread,
  persistent application layer, and Overview-window hit roles. Do not add a
  Dash.
- Keep the workspace rail fixed while the application layer changes between
  its pill-only resting depth and its expanded catalog depth. When expanded,
  it replaces and disables input for the window spread.
- Use the shared cool-slate full-output backdrop, including beneath the
  transparent StatusBar. Do not draw enclosing rail, stage, or drawer panels.
- Implement the tablet large-card Overview switcher and the Home catalog with
  its compact full-width horizontal rail, including the final `+`, without
  adding tablet-only button semantics.
- Make `Overview:Windows` and `Home:Apps` visibly distinct and keep rail targets
  stable across their immediate, non-animated transition.

### Stage 4: gesture completion

- Claim direct-touch bottom-edge navigation in SWS.
- Implement interactive Home, Overview, adjacent-workspace, and split-divider
  input with animations still disabled.
- Add Overview card drag-and-drop: dragging a live window preview onto another
  card moves that window to the target workspace using the same membership
  rules as `Super+Shift+Left` / `Super+Shift+Right`. Shrink the dragged preview
  continuously toward thumbnail scale as it approaches the workspace rail.
- Add the non-workspace `+` card for explicit creation and atomic
  create-and-move drops.
- Add pointer-hover and selected-touch removal controls plus configurable
  create/remove keyboard actions. Manual empty-workspace retention remains the
  default policy.

### Stage 5: animation

- Add compositor frame-clock animations only after the non-animated state
  machine, posture conversion, pointer/touch hit testing, and keyboard paths
  pass their acceptance tests.
- Animate the application-layer depth change while keeping the workspace rail
  fixed, and fade/clip the window spread only as the catalog replaces it. Also
  animate adjacent-workspace and split-divider transitions using compositor
  presentation transforms.
- Drive tablet bottom-edge horizontal switching, upward Home entry, and
  hold-to-Switcher transitions from continuous gesture progress and velocity.
  The persistent home indicator is the gesture origin, not a fourth shell
  presentation or a clickable button.

### Stage 6: hardware and multi-output completion

- Add production switch/touchscreen device producers.
- Add per-output workspace/experience state.
- Add rotation, on-screen keyboard coordination, accessibility navigation, and
  end-to-end hardware tests.

## Acceptance Criteria

The shell implementation is not complete merely because posture detection
changes a flag. A release candidate must demonstrate:

1. Entering tablet posture never marks ordinary apps maximized as policy.
2. Laptop Overview simultaneously shows a stable workspace rail and a
   non-overlapping spread of the selected workspace's windows; Apps visibly
   replaces that spread on the same canvas without moving the rail. No Dash or
   enclosing drawer panel is present.
3. Tablet Overview is a dedicated large-card `Single`/`Split` switcher with no
   catalog. Home shows the app catalog with a compact complete-card rail and
   selectable `+`; the home indicator remains visible by default.
4. Entering either Overview depth cannot alter workspace membership by itself.
5. Workspace switching preserves app buffers and does not flash blank frames.
6. Returning to Desktop restores prior freeform geometry.
7. A direct-touch system gesture never leaks a partial click to an app.
8. Split ratio changes are atomic and survive a posture round-trip.
9. Shell restart preserves live workspaces and apps.
10. Every primary tablet action has a keyboard/pointer equivalent.
11. Invalid or stale shell transactions have no partial effect.
12. Empty workspaces survive closing and moving their final scene; explicit
    creation/removal and create-and-move drops publish one atomic generation.
13. Removing a workspace never terminates a scene and rejects unsafe tablet
    merges.
14. RISC-V and AArch64 builds, formatting, protocol tests, workspace model
    tests, and compositor policy tests pass.
