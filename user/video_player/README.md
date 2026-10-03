# video-player

Scarlet-native media player.

This crate is built separately from `user/bin` so codec support can be selected
per build or per filesystem bundle.

## Default Features

The crate default is:

```toml
default = ["av1-stateful-hw", "h264-stateful-hw", "mp4-aac"]
```

That enables MP4/AAC playback, stateful AV1 hardware decode, and the stateful
H.264 hardware decoder path. For H.264 decode, the default path is stateful
hardware decode only.

At runtime, `video-player` also defaults to hardware decode. Pass
`--software` (or `--swdec`) to explicitly select the software path.

## Opt-In Codec Paths

- `h264-stateless-hw` enables userspace H.264 request building through
  `scarlet-codecs/h264`.
- `h264-sw` enables the software H.264 decoder dependency.
- `vp9-stateless-hw` enables userspace VP9 request building through
  `scarlet-codecs/vp9`.
- `vp9-stateful-hw` and `hevc-stateful-hw` reserve stateful hardware decode
  paths.

H.264/AVC may be patent-encumbered in some jurisdictions. Stateless and
software H.264 support are therefore explicit opt-ins.

Enabling a codec feature does not grant or provide any codec patent licenses.
Distributors and users are responsible for supplying any licenses or permissions
required in their jurisdiction.

## Controls and fullscreen

The overlay follows Scarlet's Normal/Tablet posture: compact 28px controls in
Normal, 44px touch targets in Tablet, with the same centered 16px native icons.

- Confirm toggles the focused action. Up/Down or Tab moves between playback,
  loop, seek, and fullscreen. Left/Right seeks five seconds.
- The lower-right button or F11 toggles fullscreen. Escape/Cancel first leaves
  fullscreen; outside fullscreen it closes the player. Held Confirm/F11/Cancel
  cannot repeatedly toggle or close immediately after leaving fullscreen.
- Space/P toggles playback, L toggles loop, Home/End seeks to the ends, and D
  toggles the existing diagnostics.
- Pointer/touch dragging previews a seek and commits on release. Cancellation,
  resizing, or posture changes discard the uncommitted seek. A first tap on
  hidden controls reveals them without activating a button. Tapping the video
  area with touch toggles the overlay; swipes and cancelled contacts do not.
- During playback the overlay hides after roughly four seconds without input,
  including after keyboard/controller navigation. Stationary pointer updates
  do not extend this delay. Pause, an active touch/seek, or held Confirm keeps
  controls visible; release/cancellation starts a fresh delay. Touch can also
  dismiss the overlay while paused.

Fullscreen uses the window server's fullscreen API and confirmed state, hides
the native decorations, and restores the preceding geometry on exit. Playback
position, pause/loop state, and control focus remain intact.

[Screenshots and validation](docs/controls.md)

### Host control tests

```sh
python3 user/video_player/tests/run_controls.py
```

Run from the repository root with a compatible Rust toolchain. The script uses
rustc's host target (or `--target`), the same pinned ScarletUI revision as the
player, and a generated package under `target`/`CARGO_TARGET_DIR`. It extracts the
production control/input helpers and tests them with platform I/O stubbed; media
I/O and actual hardware decode require a running Scarlet system.
