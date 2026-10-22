# Screenshot

Screenshot uses the existing exclusive SWS remote capture service for a one-shot
capture. It briefly minimizes its own window, requests output 0, reads a bounded
frame, restores the window and closes its capture connection. It never injects
input or takes over another client's session. An occupied service or unavailable
capture path produces a bounded timeout/error, leaving the other session alone.

Select a region by dragging the preview, or choose Draw arrow for a simple red
annotation. Full screen restores the full selection; Reset edits restores the
original capture. PNG exports the selected pixels. Ctrl+N captures with an
unsaved-change guard; Ctrl+S saves. Existing regular-file replacement requires
confirmation. Links and special destinations are rejected. New outputs use
exclusive creation; replacements stage and synchronize data before rename.
Failed cleanup reports the retained path. Pixel dimensions and transport payloads
are bounded, and capture/save work runs outside the UI event handler.

`--preview path.png` loads an explicit test image and labels it as a fixture.
It does not simulate a successful screen capture. Host native capture is
unavailable. Host Winit/SGFX smoke tests on 2026-10-22 exercised region selection,
arrow annotation, Ctrl+N guarding, Ctrl+S from the focused path field, actual
243×163 PNG output in a scratch directory, replacement confirmation and 760×540
minimum-size layout. Ten focused tests covered pixel/geometry bounds, crop,
arrow drawing, PNG collision/replacement, partial transport reads/oversized
frames, busy-state guarding and mounted shortcut delivery.

Current Scarlet Rust 26d00a8 native cross-compilation passed. Actual capture,
GPU/shared-memory coherency, minimizing/restoring and coexistence with an active
remote session still require a booted Scarlet runtime test.
