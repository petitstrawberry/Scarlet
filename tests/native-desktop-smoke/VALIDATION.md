# Native desktop validation, 2026-10-22

Environment: Scarlet Rust `26d00a8`, native AArch64 standard library, unwrapped
Clang 21.1.8 through `TARGET_CC`, QEMU 11.0.1 AArch64 TCG. No target-specific global
CC override, full rootfs download, personal documents, or kernel fork was used.

## Build and boot results

- Normal `.cargo/Cargo.toml` workspace `cargo check --locked -p scarlet-std-bin
  --bins` passed for AArch64 (35.6 seconds) and RISC-V64 (30.4 seconds).
- Native release Notepad, Files, Screenshot and Task Manager linked with the
  published SGFX native configuration. Their ELF audit passed: Scarlet OS ABI,
  AArch64, `/bin/scarlet-ld`, expected dynamic-loader imports and no TLS/RELR/
  text relocations. A raw build without the generated SGFX configuration failed
  on `dlopen`/`dlsym`/`dlerror`, as expected for that incomplete invocation.
- The existing loader smoke passed before the operation-419 kernel rebuild.
  After rebuilding the same repository kernel, the separate native rename
  fixture printed `SCARLET_RENAME419_SMOKE_OK` and exited successfully in 14.772
  seconds. These are distinct runs; the earlier loader run did not test 419.
- Final current Notepad plus `scarlet-shell` built in 43.96 seconds. An isolated
  manifest selected the local UI source at clean commit
  `3a05b0239fdc1052639d6d6c69a7fe0582b24506`, subsequently published unchanged.
  Production lockfiles were not modified for that isolated build.
- The portable harness staged 41,558,614 bytes and booted the real services,
  Notepad, Files picker service and shell in its 64 MiB image. The staged source
  snapshot records Scarlet HEAD `a42eb3b3a95d81c4c201a3b6fc9b89d6bd489307` plus
  then-current app changes in its generated source/hash manifest.
- Concurrent-run protection was exercised while QEMU held the image: the second
  invocation exited before touching it, and its SHA-256 remained unchanged.

## Observed interactions

Real QEMU input and screenshots verified initial text, select-all, replacement
typing, Ctrl+S saving a tmpfs document, Ctrl+F opening Find with input focus,
literal search selecting the matching text, Escape returning to the editor,
Ctrl+N showing Save/Discard/Cancel after an edit, and Cancel retaining that edit.
The native shortcut tests originally exposed control-code translation in SWS;
UI commit `b6a9a89` corrects it, with four host regression tests passing.

The final no-toolbar app was captured with the real global File/Edit/View menus.
Clicking File showed New/Open/Save/Save As/Close; clicking New produced a blank
Untitled document and the `New document` status. The native menu path therefore
was invoked, not merely drawn. Earlier screenshots with the temporary toolbar
are superseded by this final menu capture.

The minimal overlay root rejected replacing rename while tmpfs succeeded;
write and sync succeeded on both. See [the driver audit](OVERLAY_RENAME_AUDIT.md).
On failure Notepad retained the original and reported the staging path. Existing
global application-label behavior is documented in the README. The guest used a
plain virtio GPU display; this run does not assert hardware acceleration, native
scrolling performance, persistent-storage durability, or a full desktop image
validation. TCG input processing sometimes lagged queued input; checks waited
for the expected visible state.

## Artifact identity

| Artifact | SHA-256 |
| --- | --- |
| Actual post-419 kernel | `8d4819e6d4586cb02a8e5c90021e1b667de643b399bafc827d2a7b259abb945e` |
| Final native Notepad | `105ad92538ecf14a9fb9c28d954a3136b64dc2e652ac5ad0d1856a39c51f9f0f` |
| Native global-menu shell | `958e53897a4430a934531989f50d7f1b3a03682fc3a6394ccd1757cd0ca354de` |

Session artifacts were kept outside the repository under
`/workspace/scratch/native-boot`: `rename419-qemu/result.json`, serial logs,
`final-menu-qemu/commands.json`, `final-menu-qemu/initial.png`, and
`final-menu-staging-2-provenance.json`. Final interaction screenshots are
`notepad-native-final-file-menu-delayed.png` and
`notepad-native-final-menu-new.png` under `/workspace/scratch/screenshots`.
The parent review also uploaded the final menu image to the user's library.
Rerunning the harness generates equivalent commands, hashes and screenshots at
the operator's chosen output paths; screenshots are not committed as source.
