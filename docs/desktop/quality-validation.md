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

| Area | Actual host runtime exercised | Still requires Scarlet runtime |
| --- | --- | --- |
| Calculator | Keyboard evaluation, division error, history and converter | Cross-app clipboard |
| Notepad | Open/save guards, search/replace, undo/redo, resize | Native file picker, IME and filesystem integration |
| Clock / Calendar | Timer/stopwatch, date navigation, focus and minimum size | Native time service behavior |
| Task Manager | Resize/minimum size; graph fixture separately labeled | Live metrics and SGFX hardware rendering |
| Files | Temporary-fixture model/operation tests; no host GUI claim | GUI navigation and native filesystem operations |
| Settings | Helper tests and native compilation; no host GUI claim | Real service persistence and system information |
| Screenshot | Explicit preview fixture, region/arrow, PNG save and guards | Actual SWS capture and remote-session coexistence |
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

## Final focused suite

The final current-toolchain desktop run passed 158 tests, with two performance
benchmarks intentionally ignored in the ordinary run: Applications 28,
Calculator 13, Calendar 7, Characters 5, Clock 8, Logs 9, Notepad 26, Screenshot 10,
Task Manager 17 and Files 35. All ten production application targets passed AArch64 and RISC-V Scarlet checks
with the published UI/SGFX dependency pins (no adjacent UI override).
Settings/settingsd and the complete SWS daemon passed their separate AArch64
current-toolchain checks. Vellum passed 31 host tests and both native target
checks with its published pins. ScarletUI core passed 478 tests and the SGFX
renderer passed 90 tests; native std and legacy SWS modes were cross-checked.

Notepad opens regular UTF-8 files up to 16 MiB, checks metadata before allocation
and bounds the actual read if a file grows. Its expanded text layout caches and
edit history still consume more memory than raw UTF-8 bytes. A failed native
Notepad/Screenshot save conservatively retains partial output and names it in
the error, because conditional unlink is unavailable. Existing replacements
stage before rename; new destinations are reserved exclusively and are visible
while being written.


## Reproducible integration and review

The application implementation pins ScarletUI
`75786c4941ae407f2ae898d1942ca3a4b2c2f0df`, SGFX
`bfd51791aada00ecd93d797c5646e78b5511288f`, and the Adreno backend
`9746d05ac1879e3e5dbf97da3fe60b59d7df0515`. Their Scarlet library dependency
is aligned at the published clipboard foundation
`74b92fe8c43ba71467dd70083411c4574de01079`, avoiding duplicate native runtime
and handle types. The Vellum bundle pins
`a54677d7e326b96c490b61bc7313f963b9cb5d58`.

Normal workspace dependency resolution succeeded and its lockfile is committed.
The normal command `cargo check --locked --manifest-path .cargo/Cargo.toml -p
scarlet-std-bin --bin calculator --target aarch64-unknown-scarlet` stopped in the
existing `ring` C build: this cloud shell did not set a target C compiler and
used host GCC, whose `_FORTIFY_SOURCE` warning at `-O0` was treated as an error.
That check did not reach application compilation. The focused native checks
above compile actual production sources while excluding unrelated network/media
dependencies. A full image build and boot remain unverified.

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
