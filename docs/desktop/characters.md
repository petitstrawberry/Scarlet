# Characters

Characters provides 118 common punctuation marks, mathematical symbols,
currencies, arrows and symbols/emoji, plus direct `U+XXXX` lookup of valid
non-control Unicode scalar values. The bundled name list is intentionally
limited; it is not a complete Unicode name database. Search accepts names,
literal symbols and code points. Every tile includes a code-point caption, and
the selected item displays all code points in a multi-scalar sequence.

The grid adapts to the window width. Arrow keys, Home/End and Page Up/Down move
the selection and keep it visible. Enter, the Copy button and Ctrl/Cmd+C copy
the selection. Up to eight successfully copied entries appear in Recent copies.
Copying preserves the exact UTF-8 sequence, even if selection or search changes
while the asynchronous clipboard operation is pending. Errors leave recent
copies unchanged and remain visible in the status area.

Native SWS builds use the session text clipboard through ScarletUI's
window-bound service. The host Winit backend currently reports that clipboard
access is unavailable. It does not substitute an application buffer. The
native protocol, authorization and limits are documented in
[`tests/clipboard-protocol/README.md`](../../tests/clipboard-protocol/README.md).

Glyph coverage depends on the installed fonts. Some emoji may appear as missing
glyph boxes, and the scalar renderer does not yet shape joined emoji into a
single glyph. Code-point labels and copied text remain intact in these cases.

## Validation

The focused harness compiles the production app source:

```sh
/workspace/cloud-setup/bin/dev scarlet-ui cargo test --manifest-path /workspace/Scarlet/tests/desktop-apps/Cargo.toml --config /workspace/scratch/desktop-ui.toml --bin characters
/workspace/cloud-setup/bin/dev scarlet-ui cargo check --manifest-path /workspace/Scarlet/tests/desktop-apps/Cargo.toml --config /workspace/scratch/desktop-ui.toml --target aarch64-unknown-scarlet --bin characters
```

Five tests cover catalogue uniqueness and literal lookup, name/code-point and
sequence lookup, invalid scalar boundaries, keyboard selection after filtering,
pending-copy selection changes and rejected clipboard writes.
All five passed on 2026-10-22 (`/tmp/characters-tests-final.log`), and the current
Scarlet toolchain's AArch64 check passed (`/tmp/characters-native-final.log`).

Host Winit/SGFX smoke checks exercised initial search focus, `U+3042` lookup,
the explicit unsupported clipboard outcome, keyboard End with selection reveal,
and resizing from 700×720 to the 480×580 minimum. Screenshots were captured under
`/workspace/scratch/screenshots/characters-*.png`. These checks exercise the host
UI; the native cross-build does not demonstrate clipboard delivery between apps
inside a running Scarlet session.
