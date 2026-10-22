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

The initial minimal overlay root rejected replacing rename while tmpfs succeeded;
write and sync succeeded on both. See [the driver audit](OVERLAY_RENAME_AUDIT.md).
On failure Notepad retained the original and reported the staging path. The final
guarded overlay fixture subsequently passed as recorded below. Captured global
application-label behavior is documented in the README. The guest used a
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


## Final overlay and PNG fixture results

The new `tests/native-overlay-save-smoke` fixture failed as expected on the
pre-overlay kernel at the first replacing rename (8.656 seconds). The final
guarded kernel passed in 11.807 seconds with `SCARLET_OVERLAY_SAVE_SMOKE_OK` and
QEMU exit status zero. This includes two production Notepad staged saves,
retained source/replaced destination handles, reopened content and inode
identity, no-replace collision preservation, and rejection of unsupported
lower-backed sources, directories and whiteouts. The final source snapshot
preceded a comments-only change in the driver; its functional mount-boundary
guard was included. The independent kernel suite also passed all 1544 tests,
including six new overlay regressions.

The production Screenshot PNG worker fixture passed in 8.896 seconds with
`SCARLET_PNG_WORKER_SMOKE_OK`, including encode/decode pixel checks, collision
preservation and error paths on the explicit 2 MiB worker stack.

| Final overlay input | SHA-256 |
| --- | --- |
| Kernel | `63abb4b7b3c5e30dcc0d04495a45931cad2492d1f53f23bba0349f9157b41878` |
| Initramfs | `20a9807543011e0dca221d773e4b376757536855c2fe0701d21532d055703e1c` |
| Native fixture ELF | `a1e47291386e1f46e6831f3ba8445ac8e43523bb0d640d069d4d53dc6bc387da` |

Exact commands, serial output and result JSON remain in
`/workspace/scratch/native-boot/overlay-save-final-qemu`; original failure logs
are in `overlay-save-before-qemu`, and PNG results are in `png-qemu`.

## Handoff boundary

An isolated native SWS/shell/Notepad/Files release build completed in 1 minute
32 seconds with the first title-propagation patch. Review subsequently tightened
active-window routing and UTF-8 title clipping. The user requested Mac handoff
before another build or GUI run. Final title commits are UI
`ab114f399944d5a554a80a1b23c2c58615acafb6` and Scarlet
`4ffda5fc10fadefe5e2e1b53e984030e3539fa43`, locally committed without pushing.
Their protocol/model tests and native checks passed, but they have no final
native visual verification here. Menu-triggered Open/Save/Find and title updates
on Open, Save As, New, focus changes and same-app background menu/close activity
remain the next native GUI checks. Earlier shortcut checks and menu-triggered
New remain valid. No additional Task Manager live-metrics guest was launched.
