# Settings view fixture

Run the host worker, mounted UI, and layout checks from the repository root.

```sh
cargo test --locked --manifest-path tests/settings-preview/Cargo.toml
```

The in-memory Bluetooth broker starts with connected headphones and a named, offline gamepad. Searching discovers a keyboard. The tests exercise keyboard pairing, address-specific actions, progress, stale actions, Forget confirmation and adapter loss. A navigation test leaves Bluetooth open while the fixture connects and disconnects the gamepad, then checks that it moves between groups without a manual refresh or inquiry.

The Winit preview imports the production Settings pages and uses in-memory service callbacks. Display and Network use empty providers. A banner identifies the fixture window.

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
