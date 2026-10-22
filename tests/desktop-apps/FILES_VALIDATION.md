# Files validation and supported behavior

The production sources are `user/std-bin/src/filer.rs`, `filer_details.rs`,
`filer_listing.rs`, `filer_operations.rs`, `filer_archive.rs`, and `filer_usage.rs`.
The shared filesystem adapter is `desktop_utilities/native_fs.rs`.

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
- Native `VfsRename` is an atomic **replacing** rename. The kernel has an
  internal no-replace variant, but no native syscall exposes it. Files disables
  Cut and Rename; keyboard attempts explain the restriction and preserve the
  original. Copy and Paste remain available. Host move/rename tests cover the
  documented copy/verify/delete fallback, not an atomic rename implementation.
- Native path-based recursive deletion cannot prevent ancestor replacement
  without directory-relative unlink operations. A selected regular file,
  symbolic link or empty directory can be removed. Nonempty directory deletion
  fails without deleting contents. Deletion still requires explicit confirmation.
- If copying or extraction fails after creating a nonempty destination, native
  cleanup may be unavailable for that same reason. The error names the partial
  destination and says it remains. It never reports successful cleanup when
  cleanup failed. Existing destinations are never replaced.

The current native build must be run on Scarlet to validate actual syscall,
window-manager, keyboard/focus, file-picker IPC and filesystem behavior. Host
fixtures and cross compilation do not replace that smoke test.

## Archives and folder sizes

ZIP (stored/deflate), TAR (USTAR, PAX paths/sizes and GNU long names), TAR.GZ and
TGZ are supported. ZIP64, split volumes, encryption, archive links, sparse files
and device/special entries fail explicitly. Extraction is into a named new
folder; it does not merge into existing directories. Extracted files use safe
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

From the supplied Nix environment, with the local UI patch config:

```sh
cargo test --manifest-path tests/desktop-apps/Cargo.toml \
  --config /workspace/scratch/desktop-ui.toml --test files_data
cargo check --manifest-path tests/desktop-apps/Cargo.toml \
  --config /workspace/scratch/desktop-ui.toml --features native-desktop \
  --target aarch64-unknown-scarlet --bin files
```

The model tests use temporary fixtures only. They cover collision preservation,
source-change verification independent of timestamps, symlink copy/delete,
symlink-aware `..` resolution, bounds, sorting/filtering, safe archive paths,
links, duplicate paths, PAX/GNU names, payload corruption, gzip trailers,
truncation, size bombs, changed archives, cancellation and non-following usage.

GridView tests in the UI repository exercise selection reveal through adaptive
five-to-two-column resize, final-row visibility, manual scrolling, a changed
selection in the same row, and retained virtualization with 50,000 cells.
The focused GridView suite passed 11 tests and ScrollView suite passed 20 tests
before the toolchain update; rerun them if the shared scrolling code changes.

## Sorting measurement (2026-10-22)

An ignored test compares the old comparator, which lowercases names during each
comparison, against the actual cached-key sorting function. Both use the same
20,000 shuffled mixed-case/Unicode names and folder flags; ordering equality is
asserted. Seven runs are measured after fixture allocation, in optimized host
code. The median in this workspace was **162.352 ms before / 9.514 ms after
(17.06×)**. This is a CPU sorting microbenchmark, not a native filesystem or GUI
latency measurement.

```sh
rustc --test --edition 2024 -O user/std-bin/src/filer_listing.rs \
  -o /tmp/filer-listing-benchmark
/tmp/filer-listing-benchmark --ignored --nocapture
```
