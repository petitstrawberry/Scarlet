# Console shell mode

ScarletShell can start with a full-screen, button-oriented Home inspired by the
console UI prototype. The default desktop drawer remains available.

```sh
scarlet-shell --mode console
scarlet-shell --mode desktop
scarlet-shell --help
```

A ready-made project is available at
[`projects/aarch64-limine-console`](../../projects/aarch64-limine-console/README.md):

```sh
cargo make run-aarch64-console
```

For a supervised desktop session, select the mode on the session launcher. It
preserves this argument when the shell restarts:

```sh
scarlet-desktop --shell-mode console
```

Select the argument on the existing session's launch command; the session owns
one shell process. SWS, stemd's application catalog, and the usual desktop
services must be running. Rebuild `scarlet-shell`, `stemd`, and `sws` for console Home;
the compositor change suppresses the desktop workspace rail on this surface.

## Home and controls

- Recently Used records successful launches and observed application activation
  during this shell session. An empty history does not reserve an empty shelf.
  History is not persisted across shell restarts.
- Library uses stemd's installed application catalog. Tiles launch or focus apps
  through the existing SWS activation-token path. Home selects the most recent
  app, or the first Library app when no recent history is available. Selection
  waits for the initial asynchronous catalog load. If sbus is not ready during
  boot, stemd keeps its existing reconnection handler running so the catalog
  service can recover.
- Workspace buttons select real SWS workspace IDs. A faint Scarlet tint over the
  shared backdrop blur and an accent outline mark the active workspace; a white
  outline marks keyboard selection independently.
  The rail reveals the selected workspace when it overflows.
- Pointer movement does not change console selection. Clicking an app or control
  still selects and activates it; keyboard navigation retains its position while
  the pointer moves across the menu or Control Center.
- Recently Used and Library remember their own selection and horizontal scroll
  position. Moving between shelves does not remount either ScrollView or reset
  a manually scrolled rail.
- Quick Settings opens the console Control Center or the existing Settings app.
  Volume and mute reflect SAS; unavailable audio is shown explicitly.
- Restart and Power Off require a separate confirmation selection. Cancel is
  selected first, so repeating Enter cannot confirm a newly armed power action.

| Key | Action on console Home |
| --- | --- |
| Arrow keys | Move selection in the displayed direction and reveal the selected tile |
| Down from Library | Enter the floating bottom controls |
| Left / Right in the action row | Move between Power, Volume, Settings, and workspaces |
| Up from the action row | Return to the previously selected app and its page |
| Enter | Open the selected app or activate the control |
| Escape | Cancel confirmation, close Control Center, or return to Workspace |
| Q / E | Switch to the previous / next workspace |
| M | Open / close console Control Center |
| Super | Toggle console Home and the current workspace |

Console mode uses Home as its switcher; requests for Overview enter Home
directly, without rendering the desktop drawer in between. Tab has no console
action. Desktop mode retains its existing Overview navigation.
Pointer clicks and native horizontal scrolling also work. Direct gamepad input
is not yet exposed by the current ScarletUI event model; these keys are the
mapping targets for a controller adapter. The UI does not claim a connected
controller or display invented device status.

## Layout and rendering

The approved B layout gives the application shelves the full screen:
Recently Used above Library, with Power, Volume, Settings, and workspaces
floating over the bottom. There is no sidebar or selected-application panel.
The vertical viewport reaches all four screen edges, including behind the
separate status window. Each horizontal shelf also reaches both side edges.
Margins and the initial status/control clearances belong to the scrolling
content, so artwork can move behind the chrome instead of disappearing at an
inset boundary. Keyboard navigation reveals the selected shelf in the safe
area between the status bar and controls.

Each system button and the workspace rail use ScarletUI floating surfaces.
The bottom controls live in a transparent, keyboard-focusless `SHELL_PANEL`
surface. Its explicit rounded input regions cover only the controls, so gaps
still deliver pointer and wheel input to the application ScrollViews beneath.
Home retains keyboard focus and owns application activation tokens. Opening
Control Center removes the floating controls and their input regions until the
panel closes. There is no permanent operation guide; scrollbars are hidden.
Power uses Scarlet red; Volume and Settings use the same opaque white as their
labels. Launch errors appear in a centered toast sized to its text plus padding,
capped by the output width.

The status bar and floating controls declare the same backdrop material through
ScarletUI's surface-region API. SWS samples the composed content below each
surface, blurs it, and draws the sharp UI on top. Both software and SGFX use
three separable box filters with a three-radius sampling halo and the same
rounded output mask. The software path reuses full-resolution buffers; SGFX
uses cached textures, at most 2× downsampling, and paired linear taps, without
GPU readback. Regions at the same layer capture their sources before any blur
is composited, avoiding feedback where halos overlap. Damage in a source halo
refreshes the complete required background before filtering. GPU passes retain
the existing tracked-frame completion rules. Normal desktop and application
status rendering keep their existing appearance.

The full-screen root repaint boundary and nested ScrollView caches remain in
use. Warm scrolling preserves the stationary controls and overlaps without
repainting every cover. Independent picture textures use separate logical SGFX
upload submissions; SGFX owns native packetization and completion.
Unchanged images, text, icons, buttons, and layout modifiers retain their render
objects during parent rebuilds. Each tile caches its artwork separately from
its selection outline. A target already visible in a ScrollView does not force
another layout. Transparent surfaces replay the ordered retained scene through
the damaged output rectangles, including ancestors and overlapping content.
This keeps alpha composition correct without committing the whole screen for
every selection change.
Console scenes explicitly subscribe to their content states through ScarletUI's
`Application::scene_listenables`. Clock, CPU, and desktop menu updates no longer
invalidate the application shelves. Audio state remains live through a separate
volume/mute state, and the minute-only clock changes only once per minute.
Floating control geometry is shared between rendering and surface regions;
window synchronization does not construct discarded controls to find their bounds.
Files and its picker use ScarletUI's `Background::hover_color` for cell hover.
This updates local paint without rebuilding the application or the virtualized
grid, and keeps click selection separate from pointer hover.

Action targets share a 44-pixel height. Native ScarletUI buttons are centered
inside those targets, including their icon and label. Below 800 logical pixels
the volume label becomes shorter; below 600 the three system actions show only
their icons. Workspaces use a horizontal rail on the right; a partial final
page remains anchored to that edge. This keeps the
same layout and directional navigation on landscape, square, and small outputs.
Tile widths adjust to the available height while retaining 16:9 images and a
readable minimum. Short outputs scroll the shelves. Control Center opens over
Home as a right side panel, using the available width on smaller displays.
Image frames stay 16:9 and every name panel stays 52 logical pixels
high. Selection changes the border, never the image size; keyboard navigation
reveals the selected shelf as well as its horizontal page.
The border and image clip share the same outer rectangle and corner radius.
ScarletUI's rectangular strokes already grow inward, so the Border modifier
must not apply an additional half-stroke inset that exposes artwork outside
the selection ring.
Down moves from Recently Used to Library and then to the bottom action row.
Left / Right follows that row through the system actions and workspaces; Up
returns to the saved app selection and horizontal page.
All geometry is in logical pixels and is recomputed after output changes.

The UI uses ScarletUI Surface, Button, Text, IconView, Image, stack, and
ScrollView components. Application covers use optional app artwork, falling
back to an enlarged and blurred launcher icon behind a crisp foreground icon.
There is no web view.
Both shell modes prefer the bundled M PLUS UI font with the terminal font as a
fallback; an explicit `SCARLET_UI_FONT_PATH` keeps ScarletUI's configured discovery.
The existing wallpaper setting and live background service are shared with the
desktop mode; the translucent Home tint lets that wallpaper show through. Horizontal
keyboard navigation reveals a page when selection leaves it, while native
horizontal wheel/touch scrolling remains available inside each rail.

Console Home uses the `CONSOLE_HOME_APP_ID` identity on a `SHELL_BACKGROUND`
surface. In Home presentation, SWS emits no workspace-card rectangles or rail
hit region for that identity, so normal app windows stay hidden and input goes
to the shell. Console Overview requests are normalized to Home before applying
surface visibility and focus.

## Session window management

While the console shell surface is registered, SWS uses its existing Focused
windowing policy. A confirmed top-level application scene fills an empty
workspace or opens a new single-scene workspace when the current one is
occupied, using the tablet launch path. Transient children stay with their
parent. Activating an existing app selects its existing workspace. Home is the
session switcher; apps use the available workspace area instead of floating
windows requiring pointer resizing.

A pending scene stays hidden in the compositor while it receives permission to
submit its first frame. Suspension and frame callbacks must both permit that
bootstrap frame, otherwise the client and scene registration wait on each other.
Once a frame is submitted, normal visibility and presentation pacing resume.
After a suspended surface becomes visible again, ScarletUI schedules a complete
presentation of its retained scene even if the application state is unchanged.
This supplies the frame SWS awaits when Home returns from a workspace, without
waiting for a key or pointer event or rebuilding the artwork caches.

This session policy does not report a tablet posture or invent controller
capabilities. SWS retains hardware posture and user windowing overrides while
console is active. Removing the console shell surface restores that policy
through the existing desktop/tablet conversion path. The normal desktop shell
continues to use its existing window-management behavior.

## Design references

The console shell retains Scarlet's wallpaper, status bar, palette, icons, and
components. Application shelves and the bottom action row do not need enclosing
panels: the individual application cards and control targets carry selection.
Network and Display do not get separate shortcuts that open the same Settings
screen. Detailed audio and power controls remain in Control Center.

The division between a launcher and on-demand system controls follows
[Nintendo's HOME and Quick Settings](https://support.nintendo.com/jp/switch2/play/use/homemenu/index.html)
and [PS5 Control Center](https://www.playstation.com/en-us/support/games/customize-ps5-control-center/).
[Xbox's Home design](https://news.xbox.com/en-us/2023/07/26/welcome-to-your-new-xbox-home/)
provides compact access to system destinations, and
[RetroArch Ozone](https://docs.libretro.com/guides/ozone/) informed the earlier
sidebar study. B uses a bottom row to reduce persistent system UI. These inform
the interaction structure; the visual components remain ScarletUI.

## Application icons and background images

Set artwork in the application's existing `/etc/stemd.d/apps/<app-id>.desktop`
entry. For example, keep the application's `Name` and `Exec` and add:

```ini
Icon=folder
X-Scarlet-Background=/share/app-art/files-cover.png
X-Scarlet-BackgroundBlur=label
```

`Icon` accepts an absolute path to a static PNG or JPEG. PNG alpha is preserved;
the same image appears in the normal launcher and console cards. Existing
symbolic names such as `folder` keep their existing icon.
`X-Scarlet-Background` specifies a separate PNG/JPEG artwork path inside the
guest filesystem. All 12 applications in the desktop bundle register their
1280×720 cover here. The image is contained in a fixed 16:9 frame; mismatched
ratios use blurred padding while preserving the complete image. These images
are independent of the system wallpaper, which continues to use the shared
wallpaper settings. The source entries live in
`bundles/desktop/fs/etc/stemd.d/apps/`, and artwork in
`bundles/desktop/fs/share/app-art/` is installed at `/share/app-art/`.

| `X-Scarlet-BackgroundBlur` | Appearance |
| --- | --- |
| `none` | Keep both the picture and its reflected name background sharp |
| `full` | Blur the picture and its reflected name background |
| `label` | Keep the picture sharp; blur the reflection behind the separate name row |
| omitted / `auto` | `label` with a dedicated background; `full` with an enlarged icon |

The selected A treatment reflects only the lower edge of the fitted artwork
downward into the name panel. It does not repeat the image from the top. The
name panel has a clear boundary and a constant dark tint; the dedicated picture
above it is untinted. Blur never affects the foreground icon or name. Without
a readable background, the shell enlarges the custom icon; without either
image, it uses the existing symbolic icon and cached cover. An invalid blur
value uses `auto`.

Files must be at most 8 MiB, 4096 pixels on either edge, and 4 megapixels total.
PNG animations are not supported. Decoding and blur happen on the catalog
worker; icons, thumbnails, and card crops are cached. Replacing an image at the
same path refreshes it on the next catalog poll when its size or modification
time changes. Restart stemd/the desktop session after editing `.desktop` fields,
because the existing registry loads those entries at startup. There is currently
no GUI editor or file picker for these per-application artwork fields.

The new `ListApplicationsWithArtwork` method returns flat groups of five
strings: app ID, display name, icon, background, blur. The original
`ListApplications` method still returns triples for existing clients. The shell
falls back to that method when connected to an older stemd.

## Build and verification

From the repository's Nix development environment:

```sh
cargo make image-aarch64-console
cargo make run-aarch64-console
```

These tasks use `projects/aarch64-limine-console` and the release configuration.
Its rootfs layer starts `scarlet-desktop --shell-mode console`, preserving the
mode on shell restarts. The console changes require the matching ScarletUI
work for cached pictures, ScrollView reveal, independent texture uploads,
aligned border bounds, surface regions, scene state dependencies, and local hover
painting. During local development these changes are in the sibling ScarletUI
working tree. This workspace's ignored
`.scarlet/cache/cargo-home/config.toml` under the console project patches the UI
and SWS crates to those local sources. A clean checkout needs the same local
patches until the published dependency revision includes these changes.

The temporary host verification harness is not part of the build or committed
project layout. It compiles the production console view and checks navigation,
pointer dispatch, asynchronous catalog loading, recent history, artwork fitting
and caching, scroll reveal, warm-scroll composition, workspace policy, and
software backdrop filtering. The latest run passed 80 checks, including renders
at landscape, square, ultrawide, and small logical sizes. The protocol suite
passed 31 unit tests and 6 integration tests, including surface-region payload
validation. ScarletUI core passed 338 tests and 23 doc tests, including scoped
invalidation, subscription teardown, retained artwork during focus changes,
partial alpha composition compared pixel-for-pixel with a fresh render,
ScrollView selection after content/viewport changes, presentation after resume,
and hover damage limited to two cells in a virtualized 200-item grid.
One existing test was ignored; the expected-panic
ID exhaustion test was filtered after the host runtime aborted while trying to
initiate its panic. `cargo make image-aarch64-console` built the standard project
image successfully with the local dependency patches above. Artwork sources are recorded in
[the prompts and provenance](../assets/app-artwork/PROMPTS.md).

Native release validation uses the console project's bundles and service
configuration in an isolated image. It covers the real 13-entry stemd catalog,
12 registered covers, Files/Notepad/Settings launches, recent-use updates,
Home/workspace switching, and Control Center. Pointer checks verify Power and
Settings activation, wheel pass-through in control gaps, and wheel blocking on
buttons. Output changes include 2560×1440, 2560×1080, 1800×1800, 1280×960, and
3840×2160 with the default 200% scale. The status bar remains sharp over the
blurred, scrolling artwork. Power remains red and the other action icons white.
The native SGFX run also checks repeated Clock/Home round trips without input
after returning, and Library/Recently Used transitions with a scrolled rail.
Home contents and the Library position remain visible without a repair input.

A temporary host benchmark renders the production application shelves with
13 catalog entries, bundled covers, three recent applications, and 200% scale.
For six warm adjacent-selection changes, median UI preparation time changed as
follows (rounded milliseconds):

| Physical output | Before | After |
| --- | ---: | ---: |
| 1280×720 | 81.9 ms | 3.1 ms |
| 2560×1440 | 157.8 ms | 4.5 ms |
| 3840×2160 | 190.5 ms | 5.6 ms |

These measurements include view reconciliation, layout, and retained CPU
picture preparation. They exclude backend rendering, SWS composition, and
display latency, and are not a guest frame-rate measurement. First rendering
and scrolling to newly visible content still have additional work.
For native compositor diagnosis, starting SWS with `SWS_PROFILE=1` logs per-frame
output/damage size, blur region and quad counts, and synchronization, encoding,
submission, and presentation durations in microseconds. It is off by default.

Output dimensions come from screen-size notifications. A per-window configure
must not replace them, and a status-bar size must not be returned as a resize
request for all scenes. Each scene synchronizes its own geometry.

Only release builds have been validated for the native session. Direct gamepad
events and audible output are outside this verification; the VM uses a silent
virtual SAS output.
