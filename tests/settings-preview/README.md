# Settings view fixture

This host-only validation program imports the production `user/bin/src/settings_views.rs` module. It exercises the same navigation, layout and controls through Winit, with a permanent fixture banner. Its model and callbacks keep changes in memory; it does not import native audio, display, SWS, SBUS, filesystem persistence or time-zone services. This is visual/interaction evidence, not a native Settings runtime test.

```sh
cargo run --manifest-path tests/settings-preview/Cargo.toml
cargo run --manifest-path tests/settings-preview/Cargo.toml -- --empty
```

The normal fixture supplies named devices and methods. `--empty` supplies missing audio/cursor/input providers and a background-save error. Test default, minimum (800×574 including the fixture banner) and enlarged windows; visit every page, use Tab/Shift+Tab, arrows/Space/Enter, and scroll Appearance and Shell & Status to their final actions. Production Settings is separately compiled by `tests/settings-app` for `aarch64-unknown-scarlet`.

With Xvfb (or a dedicated X display), `xdotool`, and ImageMagick available, the visual smoke script captures every page and exercises selection, mute, and keyboard access to the bottom of Appearance:

```sh
bash tests/settings-preview/visual_smoke.sh /path/to/settings-preview /tmp/settings-normal
bash tests/settings-preview/visual_smoke.sh /path/to/settings-preview /tmp/settings-empty --empty
```

Inspect the PNGs; a successful script verifies that the process survives the interaction sequence, not that every pixel is correct. At minimum size, the keyboard sequence is Tab to Style, PageDown three times, Shift+Tab to Save again, Enter, then Home. The save image should show the focused action and the memory-only save status; Home should restore the top of Appearance. No real configuration is written.

The October 2026 visual audit used Winit with SGFX/wgpu on Xvfb. It covered normal and missing-provider fixtures, all seven pages, minimum/default/enlarged windows, scrolling to final actions, audio selection and mute, and the keyboard sequence above. Native compilation passed separately; live native services were not exercised by this harness.
