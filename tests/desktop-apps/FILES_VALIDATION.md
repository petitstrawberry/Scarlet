# Files validation and supported behavior

The application entry point is `user/std-bin/src/filer/main.rs`. Its `archive`,
`details`, `listing`, `operations`, `usage`, `workers`, `icons` and `picker_filter`
modules live alongside it. Shared filesystem capabilities are supplied by the
`scarlet-fs` library in `user/lib/scarlet-fs`; ordinary I/O uses `std::fs`.

## Blocking native extraction regression for Mac handoff

**Native archive extraction is not ready to claim safe.** The final 2026-10-22
Scarlet/SWS run exposed a destination-collision failure specific to an existing
lower/initramfs directory. The guest already contained
`/root/Sample archive extracted/keep.txt` before Files started. Extracting the ZIP
into `Sample archive extracted` unexpectedly reported success instead of
`AlreadyExists`, allowing content into an existing destination. A second attempt
against the now-upper-layer directory correctly refused the collision. The prior
new-destination extraction and upper-layer collision checks therefore remain
passes, but they do not cover this failing lower-layer case.

The guest used the current rebuilt kernel, Scarlet Rust 26d00a8a3 and published
ScarletUI 3a05b023. Its generated fixture, boot image, initial framebuffer, command,
serial log and structured outcome are preserved in
`/workspace/scratch/native-boot/files-collision-qemu/` (`review-result.json`). The
input marker remains in `files-staging/root/Sample archive extracted/keep.txt`.
Screenshots are `files-native-lower-directory-collision-failure.png` and
`files-native-final-extract-collision.png` under the visual-review directory.
The latter shows the corrected upper-collision message; it is not evidence that
the lower-directory failure is fixed. Filesystem diagnosis/fixes are deferred to
Mac as requested; no application filesystem workaround was introduced.

## Toolchain and native boundaries

Use Scarlet Rust **26d00a8a3 or newer**, as pinned by the repository's Nix inputs.
The preloaded cloud environment originally used 9f9ef5a4: its standard library
had incomplete lstat, canonicalization, filesystem errno and fsync adapters.
Those are already fixed in 26d00a8a3. Old-toolchain type checking is not evidence
that native filesystem behavior works. The application delegates those fixed
operations to the supported standard library.

Remaining native API limitations are handled explicitly:

- `OpenOptions::create_new` still performs separate probe/create/open steps.
  The shared helper uses `VfsOpenAt` with `CREAT|EXCL|NOFOLLOW|CLOEXEC` instead.
  It owns the returned descriptor exactly once through `File::from_raw_fd`.
- Native `std::fs::Permissions` only exposes readonly state. The metadata
  helper reads the richer coarse READ/WRITE/EXECUTE flags for file creation.
  It preserves those capabilities for the owner, without claiming preservation
  of group permissions, ownership, ACLs, hard links or extended attributes.
  Existing permission changes are unsupported; a mismatch fails explicitly.
- Native `VfsRename` (409) remains a replacing rename. The new
  `VfsRenameWithStatus` (419) exposes the existing locked no-replace operation.
  Files probes support without using paths and enables Cut/Rename only when the
  kernel supports it. Older kernels retain an explicit unsupported result.
  Moves preserve inode/open-handle identity; collisions never replace targets.
  Cross-filesystem moves preserve the source and explain that Copy is required.
  Linux host tests use atomic `renameat2(NOREPLACE)`, not copy/delete emulation.
- Native path-based recursive deletion cannot prevent ancestor replacement
  without directory-relative unlink operations. A selected regular file,
  symbolic link or empty directory can be removed. Nonempty directory deletion
  fails without deleting contents. Deletion still requires explicit confirmation.
- If copying or extraction fails after creating a nonempty destination, native
  cleanup may be unavailable for that same reason. The error names the partial
  destination and says it remains. It never reports successful cleanup when
  cleanup failed. The lower-directory collision regression above blocks the
  intended native new-destination-only extraction guarantee.

The current native build must be run on Scarlet to validate actual syscall,
window-manager, keyboard/focus, file-picker IPC and filesystem behavior. Host
fixtures and cross compilation do not replace that smoke test.

Scarlet std26d still has a diagnostic-only defect in
`library/std/src/sys/pal/scarlet/os.rs:15`: `error_string` ignores errno and
always returns `operation successful`. A native second extraction correctly
returned `AlreadyExists` (errno 17), but its generic display was misleading.
Files now gives that expected collision an explicit actionable message based on
`ErrorKind`, without changing std or treating extraction as unsupported.

Files reserves a bounded 2 MiB stack for its filesystem, thumbnail and detail
workers. Scarlet std26d's default is only 64 KiB: actual native deflate-reader
construction stalled on that stack even for a 710-byte ZIP. The identical
production parser completed with the explicit worker stack. Thread creation
failure is reported in the relevant status area; busy state is cleared so the
user can retry. `tests/native-files-smoke` imports this production helper and
checks native ZIP browsing plus folder-size completion on disposable fixtures.

## Archives and folder sizes

ZIP (stored/deflate), TAR (USTAR, PAX paths/sizes and GNU long names), TAR.GZ and
TGZ are supported. ZIP64, split volumes, encryption, archive links, sparse files
and device/special entries fail explicitly. Extraction is designed for a named
new folder and rejects existing destinations in host tests. The native lower-
directory regression above violates that guarantee and remains a blocker. Extracted files use safe
creation permissions. Regular files retain owner execute when present (0700 instead
of 0600); group/world permissions, special bits and archive ownership are not restored.

Archive readers enforce 20,000 entries including implicit directories, 64
path components, 512 MiB archive input, 256 MiB per file, 1 GiB total regular-file
payload, bounded extension headers and a two-minute operation deadline. They
check cancellation between bounded reads. Paths must be relative and cannot
contain `..`, NUL, backslashes, Windows drive syntax or control characters.
Duplicate paths and file/parent conflicts fail before extraction. ZIP central
records must exactly match their bounded 16 MiB declared extent and entry count.
The library sees an immutable verified directory and minimal local headers during
metadata parsing, so it cannot fall back to unverified EOCDs in file data. All raw
reads and seeks check cancellation/time limits, including inside ZIP/gzip parsing. TAR checksums,
ZIP payload CRC and gzip trailers are checked. Partial/new output only is
eligible for cleanup; an existing destination is never removed.

Folder size uses logical regular-file sizes, counts links without following
links, and shows the largest 20 immediate children. Hard-linked file paths each
contribute their logical size. It is not an allocated-disk-space measurement.
Unreadable items and depth/item/time limits produce an explicit partial result.
The walk stops at 100,000 items, 64 levels or ten seconds and supports cancellation.

## Repeatable checks

From the repository root using the Scarlet toolchain (published dependency pins):

```sh
cargo test --manifest-path tests/desktop-apps/Cargo.toml \
  --test files_data
cargo test --manifest-path tests/desktop-apps/Cargo.toml \
  --features native-desktop --bin files
cargo check --manifest-path tests/desktop-apps/Cargo.toml \
  --features native-desktop \
  --target aarch64-unknown-scarlet --bin files
```

The model tests use temporary fixtures only. They cover collision preservation,
atomic move identity with open handles and hard links, symlink copy/delete,
symlink-aware `..` resolution, bounds, sorting/filtering, safe archive paths,
links, duplicate paths, PAX/GNU names, payload corruption, gzip trailers,
truncation, size bombs, changed archives, cancellation and non-following usage.

GridView tests in the UI repository exercise selection reveal through adaptive
five-to-two-column resize, final-row visibility, manual scrolling, a changed
selection in the same row, and retained virtualization with 50,000 cells.
The focused GridView suite passed 11 tests and ScrollView suite passed 20 tests
before the toolchain update; rerun them if the shared scrolling code changes.

The native operation 419 smoke fixture also booted on AArch64 Scarlet in QEMU
on 2026-10-22: `SCARLET_RENAME419_SMOKE_OK`, exit 0, 14.772 seconds. It exercises
the production filesystem helper, the capability probe, invalid pointers/paths,
destination collisions (including dangling links), live open handles/hard links,
source symlinks, cross-filesystem preservation and root/dot/cycle rejection.
The repeatable fixture is `tests/native-rename-smoke`; captured boot evidence is
`/workspace/scratch/native-boot/rename419-qemu/result.json` and `serial.log`.
This verifies the native primitive, not the full Files GUI or picker service.

The production Files application suite passed **58 tests, with one ignored
sorting benchmark**, using published ScarletUI `3a05b023` on 2026-10-22. The
native worker regression then passed on AArch64 Scarlet/QEMU with
`SCARLET_FILES_WORKER_SMOKE_OK`, exit 0, 18.038 seconds. The native marker includes
both ZIP browsing and folder-size completion through the actual production
worker helper; evidence is in
`/workspace/scratch/native-boot/files-worker-qemu/result.json` and `serial.log`.

## Actual-view host visual review

Host Files requires an explicitly supplied fixture directory. The same production
views run through Winit and SGFX/wgpu; the window labels itself as a host preview.
Scarlet application launching and file-picker IPC remain native-only. This mode
is a visual/interaction check, not a substitute for native GUI smoke testing.

```sh
fixture=$(python3 tests/desktop-apps/files_preview_fixture.py)
cargo build --manifest-path tests/desktop-apps/Cargo.toml \
  --features native-desktop --bin files
SCARLET_ICON_THEME_ROOT="$PWD/bundles/desktop/fs/share/icons/default" \
SCARLET_UI_WINIT_RENDERER=sgfx \
  tests/desktop-apps/target/debug/files --preview-fixture "$fixture"
```

With a shared `CARGO_TARGET_DIR`, use its `debug/files` binary instead. Final
checks use the published dependency pins without local source overrides. In this
workspace the actual host binary lives under
`/workspace/cloud-setup/targets/scarlet-ui/scarlet-host/debug/files`.

The 2026-10-22 captures under `/workspace/scratch/visual-review-files/` include
the main browser at 960×640 and its 760×520 minimum, archive navigation and
extraction at 820×620 and 680×500, and folder-size results at both detail-window
sizes. They exposed clipped long folder names and an unstructured capacity page;
the revised views wrap folder labels, align readable status messages and show
size totals with separate rows for the largest children. Actual button-driven
extraction succeeded into the disposable fixture, and a second attempt showed
the collision error without replacing output. The shared focus recovery fix was
also checked through the real Files window: clicking a grid item, pressing
Ctrl+F and typing immediately now updates the filter without a pointer click in
the field (`files-focus-corrected.png`).

Actual Scarlet/SWS captures are separately named `files-native-*.png`. The first
native boot confirmed the main browser, directory navigation and long/Unicode
filenames. It exposed the worker-stack failure described above when opening a
deflated archive; the host previews did not reveal that native problem. After
the explicit worker-stack fix, the same native Files window completed ZIP
inspection, displayed 4 entries / 58 expanded bytes and navigated into
`/Project/documents`. Long names remained on two lines above separate byte-size
labels, alongside Japanese names. The native Extract button successfully
created `/root/Sample archive extracted` in the disposable guest; a second
attempt refused that upper-layer destination. The separate pre-existing lower-
directory fixture failed as documented above. Verified
frames are `files-native-final-archive.png`,
`files-native-final-archive-longnames.png`,
`files-native-final-extract-success.png`, `files-native-final-extract-collision.png`
and `files-native-final-browser.png`.
Folder-size calculation also completed in the native GUI: `/root/Documents`
contained 1,186 bytes, 2 files and 1 folder, with child rows of 770 and 416 bytes
(`files-native-final-usage.png`). These captures use a disposable initramfs under
QEMU and a development build, so they are not hardware performance measurements
or validation of all picker IPC and filesystem operations.

## Sorting measurement (2026-10-22)

An ignored test compares the old comparator, which lowercases names during each
comparison, against the actual cached-key sorting function. Both use the same
20,000 shuffled mixed-case/Unicode names and folder flags; ordering equality is
asserted. Seven runs are measured after fixture allocation, in optimized host
code. The median in this workspace was **162.352 ms before / 9.514 ms after
(17.06×)**. This is a CPU sorting microbenchmark, not a native filesystem or GUI
latency measurement.

```sh
rustc --test --edition 2024 -O user/std-bin/src/filer/listing.rs \
  -o /tmp/filer-listing-benchmark
/tmp/filer-listing-benchmark --ignored --nocapture
```
