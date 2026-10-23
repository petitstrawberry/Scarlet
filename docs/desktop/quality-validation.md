# Desktop quality work: verification and limits

This work targets the active Scarlet `dev` base
`66c84726e0b21fc93db20df0597cac0f31d6030f`, with ScarletUI's app-pinned
`ec25336241e70d2fcdad186b3b50f8aa449d303a` as the starting toolkit.
The existing Vellum viewer is extended rather than replaced. Existing browser,
PDF viewer, music player, emulators and games remain outside this change.

## Toolchain correction

The environment originally pinned scarlet-rust-nix
`2b4ddd555389a8c53806bbc2586440b2662c38dd` and installed Rust/native std
`9f9ef5a48648bee5e9be6a336b1314714abf9b88`. That was stale relative to Scarlet.
Scarlet's production flake already pins scarlet-rust-nix
`3c23906c35053e28d56f3aa9b1d29142535ffab7`: compiler
`71dd0425890dc69e910009b4df4ead3cce17b86a`, native std
`26d00a8a3a8489d73f5ae3575880e9f915dfd0f2`.

Current native checks use cached `scarlet-rustc.out` from scarlet-rust-nix
`79f356924d1a93b55904c6b2733d0b3da5ce0de5`, reporting
`rustc 1.94.0-nightly (26d00a8a3 2026-10-13)`. This supplies compiler and native
std at 26d00a8, matching production's native standard-library source. The full
production compiler bundle was not rebuilt. No production toolchain pin was
changed to accommodate the environment.

Current source implements no-follow metadata, canonicalization, errno mapping
and fsync. Application code uses those standard-library operations. Remaining
exclusive-creation, permission and deletion boundaries are documented in
`../../tests/desktop-apps/FILES_VALIDATION.md`; old std behavior is not presented
as a current OS defect.

## Test methods

`tests/desktop-apps` compiles actual application sources and exercises calculator
parsing/conversion, document transitions, undo/redo, clipboard completion guards,
calendar arithmetic, monotonic timers, graph geometry, file operations, archive
validation, log transport and app-package transactions. Filesystem tests create
temporary fixtures. `tests/settings-app` cross-checks actual Settings/settingsd;
`tests/clipboard-protocol` tests the wire contract/session store and cross-checks
the actual SWS daemon. These focused workspaces exclude unrelated media/network
build dependencies; they are not substitutes for a full OS image build.

Host screenshots come from actual Winit windows in Xvfb. App smoke uses SGFX/wgpu on Mesa; typography and the Clock/Calendar/Logs comparison uses the explicitly selected CPU renderer with Softbuffer.
Host metrics, capture, logs, package mutation and clipboard limitations are shown
honestly. Explicit preview fixtures are labeled. Host screenshots do not prove
native compositor integration. No user applications, personal files or live
machine settings were changed for testing.

## Runtime verification matrix

| Area | Runtime evidence | Remaining native checks |
| --- | --- | --- |
| Calculator | Keyboard evaluation, division error, history and converter | Cross-app clipboard |
| Notepad | Host open/save guards, search/replace, undo/redo, resize; native document load, Ctrl+A/Ctrl+S/Ctrl+F, real File-menu New and isolated repeated overlay saves | Native menu Open/Save/Find, final title interaction matrix and IME |
| Clock / Calendar | Timer/stopwatch, date navigation, focus and minimum size | Native time service behavior |
| Task Manager | Resize/minimum size; graph fixture separately labeled | Live metrics and SGFX hardware rendering |
| Files | Host temporary fixtures and native browser/archive navigation, new-destination extraction, upper-layer collision errors and folder usage; operation 419 and worker fixtures pass | **Blocking:** existing lower/initramfs extraction destination is incorrectly accepted; full picker and remaining filesystem cases |
| Settings | Same production page views in a clearly labeled fixture harness, including minimum-size scrolling and controls | Real service persistence and system information |
| Screenshot | Host preview fixture, region/arrow, PNG save and guards; native worker exports, decodes and compares every PNG pixel | Actual SWS capture and remote-session coexistence |
| Logs | Explicit static preview, filter/details/follow controls | Live logd transport |
| Applications | Fixture inspection, errors and minimum size | Native mutation, recovery and catalog refresh |
| Characters | Search, keyboard selection and unsupported-backend errors | Native clipboard delivery |
| Vellum | Transforms, annotation, history, dirty guards and PDF navigation | Native picker/export integration |
| Typography | CPU/Softbuffer before/after at 100%, 125% and 200% | Native renderer/DPI and hardware comparison |

## Performance evidence

ScarletUI `docs/text-scrolling-performance.md` records the scroll workloads and
measurement scope and the replacement, work-verified benchmark. Historical
1.67-second wheel-only and 6033.636-ms full-path timings measured different
scopes and had incomplete provenance; they are superseded by the controlled run.
For 3,000 mixed-script lines at 640×480, five samples per mode yielded median
wheel-only 1822.955422→0.003652 ms and wheel/update/layout/paint recording
6902.559349→0.037585 ms, comparing exact commits `ec253362` and `21b75b5`.
All ten samples matched viewport positions, visible rows and text-command
content/position hashes. The separate post-typography snapshot measured
0.030148 ms for the full path, but its deliberately changed glyph metrics
prevent identical visual-output equivalence with the old typography.
These measurements use precompiled release binaries, Rust 26d00a8 and an
explicit fixed font stack, with no concurrent builds. They establish a large
reduction in repeated CPU layout work, not an end-to-end GUI speedup.
Glyph rasterization, GPU submission and frame presentation are outside the
widget CPU recording benchmark. Long-line costs and retained-cache memory
limits are documented separately.

Files' 20,000-entry cached sorting benchmark measured median 162.352 ms before
and 9.514 ms after across seven optimized host runs with identical ordering.
See `FILES_VALIDATION.md` for the harness and workload.

Task Manager's 2,000-iteration graph preparation benchmark measured CPU raster
versus SGFX mesh preparation: 180×62, 20.08→7.25µs; 900×170, 212.30→13.24µs;
1800×600, 1577.25→9.50µs. This excludes upload, GPU drawing and presentation.
The bounded history uses at most 1,446 vertices (57,840 bytes); at the largest
size the old image used 4.32 MB. Small graphs may have larger mesh payloads than
pixel buffers; no blanket bandwidth or FPS improvement is claimed.

Font sizing/width correctness and before/after font-em measurements are described
in ScarletUI `docs/TYPOGRAPHY_VALIDATION.md`. The renderer still uses scalar
layout; full complex-script shaping, bidi and ligatures remain separate work.

## Native runtime checks still required

Boot Scarlet and verify focus/Tab/IME across the changed applications; resize
Task Manager with live metrics; test SWS clipboard between applications; capture
with and without an active remote session; exercise file pickers and temporary
filesystem fixtures; query real logd/settings/system information; and validate
package catalog reload/recovery. No native runtime or hardware performance claim
is made from cross-compilation alone.

Voice recording remains blocked on the existing playback-only kernel/SAS audio
interface. The source evidence is in `utilities-validation.md`. A nonfunctional
record button is not shipped.

## Focused suite and follow-up regressions

At the published ScarletUI pin `3a05b023`, the locked desktop suite passed
233 test executions, with three benchmarks ignored in the ordinary run:
Applications 29, Calculator 24, Calendar 7, Characters 5, Clock 8, Files 58,
Logs 9, Notepad 28, Screenshot 11, Task Manager 18, and the Files data target 36.
The Files targets intentionally overlap in model coverage; 233 is not a count
of unique scenarios. This run used actual production sources without an
adjacent UI override. Follow-up worker-startup regressions passed all 31 Notepad
tests, 14 Screenshot tests and 16 Vellum UI/edit tests, plus their AArch64 checks.

The normal `.cargo` workspace passed locked checks for **all**
`scarlet-std-bin` binaries on AArch64 and RISC-V with published dependency pins.
Settings/settingsd and the complete SWS daemon also passed separate AArch64
checks. Vellum's earlier complete suite passed 31 tests and both native target
checks; the later 16-test edit run is a separate targeted suite.
ScarletUI core passed 486 unit tests and 23 doc tests with four ignored, run
serially because an existing global glyph-cache test interferes with parallel
paint tests. The SGFX renderer's earlier focused run passed 90 tests. Native
standard-library and legacy SWS feature modes were cross-checked. Subsequent
title propagation, document-core and overlay changes require their own recorded
regressions; these counts do not pre-claim those results.

Notepad opens regular UTF-8 files up to 16 MiB, checks metadata before allocation
and bounds the actual read if a file grows. Its expanded text layout caches and
edit history still consume more memory than raw UTF-8 bytes. A failed native
Notepad/Screenshot save conservatively retains partial output and names it in
the error, because conditional unlink is unavailable. Existing replacements
stage before rename; new destinations are reserved exclusively and are visible
while being written.


## Reproducible integration and review

The application implementation pins ScarletUI
`ab114f399944d5a554a80a1b23c2c58615acafb6`, SGFX
`bfd51791aada00ecd93d797c5646e78b5511288f`, and the Adreno backend
`9746d05ac1879e3e5dbf97da3fe60b59d7df0515`. Their Scarlet library dependency
is aligned at the published clipboard foundation
`74b92fe8c43ba71467dd70083411c4574de01079`, avoiding duplicate native runtime
and handle types. The Vellum bundle pins
the published editing/worker revision recorded in `bundles/desktop/bundle.toml`.
The later title-only UI pin was checked independently; the 233-execution suite
above remains explicitly associated with the earlier `3a05b023` pin.

Normal workspace dependency resolution succeeded and its lockfile is committed.
The initially failing `ring` build was an environment compiler-selection issue.
Following `docs/development/README.md`, the environment now sets `TARGET_CC` to
unwrapped Clang 21.1.8; host compilation continues to use its normal compiler.
The normal `.cargo` workspace checks for all `scarlet-std-bin` binaries then
passed on both AArch64 and RISC-V. This does not change production toolchain pins.

Release linking of Notepad, Screenshot, Files and Task Manager passed with the
actual SGFX-generated userspace linker policy. ELF inspection confirmed Scarlet
OSABI 83, `/bin/scarlet-ld`, and only the expected `dlopen`/`dlsym`/`dlerror`
imports, with no unsupported TLS, symbol-versioning or dynamic dependencies.

A fresh AArch64 kernel and loader passed the existing `tests/loader-smoke` in
QEMU 11.0.1 TCG (11.294 seconds). A subsequent kernel containing syscall 419
passed `tests/native-rename-smoke` (14.772 seconds, exit 0), exercising the
production filesystem helper, errors, collisions, symlinks, inode/open-handle
identity and cross-filesystem source preservation. These use isolated 64 MiB
images, not the user's filesystem or the full production desktop image.
A minimal SWS desktop also boots the real Notepad and Files windows. The native
Notepad run exercises document load, selection, save on writable `/tmp`, Find
via shortcut and New through the real shell menu API. This does not yet prove
every menu action or IME behavior. The separate overlay fixture below covers
production saves over packaged lower files.
`tests/native-desktop-smoke` records staging hashes, QEMU commands and captures.

`tests/native-files-smoke` passes in the guest (18.038 seconds), loading a ZIP
and calculating folder usage through the production worker path. Investigation
reproduced a failure with the native default 64 KiB stack; the explicit bounded
2 MiB Files worker completes that fixture. `tests/native-png-smoke` also passes
(8.896 seconds), exporting and decoding PNGs with pixel equality, collision and
failure checks. This establishes codec behavior on its explicit worker stack,
not screen capture or a measured minimum stack requirement. Worker start errors
are returned to the matching UI operation instead of leaving a permanent busy
state. Vellum's existing PDF rendering queue is outside that worker change.

The final guarded OverlayFS kernel passed all 1,544 AArch64 kernel tests,
including six new regressions. The isolated native save fixture failed at the
first replacement before the change (8.656 seconds) and passed afterward
(11.807 seconds). It invokes Notepad's production save helper twice and checks
retained handles, inode identity, reopen behavior and rejected cases. Support
is deliberately limited to adjacent single-link, pure-upper regular sources on
a TmpFS upper layer; general lower-source copy-up/whiteout moves remain rejected.
Exact hashes and commands are in `tests/native-desktop-smoke/VALIDATION.md`.

During final Files handoff verification, a separate lower-directory collision
defect was reproduced: extraction accepted a destination already packaged in
the lower initramfs, while an existing upper destination was refused. This is a
blocking safety issue, not a passing extraction result. Its fixture and next
steps are documented in `tests/desktop-apps/FILES_VALIDATION.md`.

At the earlier published Scarlet head `afacda86`, CI run 39772145752 passed the
Nix-cache and Docker-image jobs. Its format failure covers the same 14 untouched
kernel files as base `66c84726`; AArch64/RV64 jobs fail before application checks
because the integration step expects an absent release kernel ELF; RV32 fails
on the same unsupported kernel `AtomicU64` use as the base. Those baseline
failures were compared against run 39519263399. The next published Scarlet head
`0cee46ab` (run 39781666499) repeats the exact same format locations, missing
release-kernel paths and three RV32 AtomicU64 errors. Its Nix-cache job passes;
Docker was still running at the recorded inspection. Adreno head `9746d05` passed all
four checks (run 39769393650). UI/SGFX workflows select PRs targeting `main`, so
the existing performance-branch reviews had no runs; Vellum has no configured
workflows. Absence of CI is not a passing result. Recheck newly published heads
before treating these historical statuses as current.

Draft reviews: [Scarlet](https://github.com/petitstrawberry/Scarlet/pull/600),
[ScarletUI](https://github.com/petitstrawberry/scarlet-ui/pull/43),
[Vellum](https://github.com/petitstrawberry/vellum/pull/4),
[SGFX dependency alignment](https://github.com/petitstrawberry/sgfx/pull/18), and
[Adreno dependency alignment](https://github.com/petitstrawberry/scarlet-project-chromebook/pull/6).
UI, SGFX and Adreno reviews target their existing `perf/shared-ir-recording`
branches; Scarlet targets `dev`. Nothing was merged or deployed.

An Astra ultra review pass completed and its findings were corrected, including
mounted screenshot keyboard handling and bounded package discovery/parsing.
A later archive-focused Astra follow-up was interrupted by the platform's
content flag; that follow-up is not represented as completed. The independently
reproduced ZIP directory-bounds defect was fixed and its regression passes.

## Visual review continuation

The initial calculator presentation was rejected and is not an accepted design.
The follow-up rebuild uses a borderless editable expression, a dominant result,
a grouped five-row keypad, hidden advanced functions and a separate history
view. Conversion uses paired value/unit rows and an actual numeric keypad.
Long-expression, selection, minimum-size and keyboard cases are part of the
review; short-expression screenshots alone were insufficient and exposed real
layout/editing defects during follow-up.

All changed applications have actual default/minimum/resized host captures,
including empty/error and keyboard states. A 15-view contact sheet covers all
12 applications and labels native guest views separately from host/fixture
views. Clock, Calendar, Logs and
Characters have separate app-sized visual refinement commits. Settings fixture
pages reuse production views, with native service mutation omitted from the
fixture harness. Applications and Screenshot show their host restrictions
explicitly. Notepad, Task Manager, Files and Vellum have actual interaction
captures. This evidence documents the review process, not user acceptance or a
claim of commercial-quality completion. The follow-up Astra ultra visual review
also verified fixes for calculator partial-expression editing and mode focus,
Notepad's narrow status line, and Vellum's focused Space-key action. Native
Notepad's redundant header is removed; its File/Edit/View menus use the existing
MenuItem API. Shell title synchronization is implemented with existing protocol
messages and preserves app identity; 28 focused UI tests and 16 protocol/model
tests passed, with native shell/SWS compilation. The final native GUI matrix is
pending at the user's requested Mac handoff. The cloud checkout also preserves
an uncommitted, incomplete TextDocument rewrite; it is not part of the published
UI revision and has no after-benchmark or passing-build claim.


## Kureha selection and workbench refinement — 2026-10-23

The desktop bundle pins published Kureha `b91abc8297bc373ce5e3617323cbd5cdef3d9403`,
which uses published ScarletUI `8fb4c6715379e18b7266764f748ff0ba14602dc6`.
No sibling bundle override or local Cargo patch is needed.

TextView now retains committed scroll offsets across intermediate parent
measurements and suppresses hover requests during selection drags. The mounted
Kureha test preserves document identity, geometry and scroll during selection,
position-status updates, and Find opening/closing. macOS forward/reverse drags
and Scarlet native selection around lines 132–146 retained their display position.
Find overlays the document instead of reducing its height. The borderless editor,
softened dark colors and compact fixed-density minimap use the same interface
on both platforms.

The host suite passed 56 library and 26 application tests; the optional real
rust-analyzer application integration passed separately. That server ran on macOS;
no language-server executable is bundled for Scarlet. Native QEMU interaction
verified /bin/sh in a real PTY and plain `ls` with the service’s initially absent
PATH. The backend supplies a default only when PATH is missing or empty.

The native `.app` build and the user’s full command both completed successfully:
`cargo scarlet image --project projects/aarch64-limine-full --release`.
Actual screenshot/input/log evidence is in `artifacts/kureha-code-20261023/`,
including `selection-find-overlay-macos.png`, `native-refinement-final/` and
`native-terminal-path-final/`. `selection-full-image-final.log` records the
published-source image build. Owned QEMU staging/images were removed after
preserving evidence; live host windows used by the user were retained.

## Kureha workbench and Storage (2026-10-23)

Kureha now offers right/down editor groups, Quick Open, Command Palette,
line/column navigation, project search and buffered replace, Save All, indentation,
line comments/move/copy, formatting and references. Same-document groups share
history and transform selections through the engine's disjoint edits; closing
or loading tabs preserves the other group's active file. Current-file rename is
supported; cross-file rename is rejected atomically and is still pending.

Published sources: Kureha `288fb58b0aa428b88d350bbc2e18a5ed65f060cb`, using
ScarletUI `01bc36e6326b19f118cbf4dc1e56f3bcb6786631` (native macOS menus and themed
TextField placeholders). Production bundles use the remote Kureha pin.

The host library suite passed 93 tests; the final application suite passed 60.
Two real rust-analyzer application tests additionally passed definition,
completion, diagnostics, formatting, current-file rename and unopened-reference
navigation. Native Scarlet runs verified both splits, independent scroll,
workspace search, 113 replacements across three fixture files, Save All and saved
file reopening. macOS verified native menu operation and splitting before the
screen locked. Native LSP execution remains unverified without a native server.
Final interaction regressions cover returning focus after Replace All and keeping
queued Quick Open characters out of the previous document.

Storage has its own std-bin main and a native `.app` recipe and icon. Ten model
tests and the final script-built AArch64 package passed; native operations verified
ranked sizes, drilldown/back and the system folder chooser. See utilities-validation.md.
Evidence and screenshots are under `artifacts/kureha-workbench-20261023/` and
`artifacts/storage-20261023/final-native/`. These checkpoints do not establish full
VS Code parity: nested groups, multi-cursor editing, folding, Git/diff, debugging,
extensions and workspace restoration remain outside the current implementation.

`cargo scarlet image --project projects/aarch64-limine-full --release` completed
successfully through the existing Nix development shell at the published Kureha
288fb58 pin, producing `.scarlet/images/limine-aarch64-full.img`. The lock was
regenerated from that build, including Storage and the updated Kureha app hashes.
