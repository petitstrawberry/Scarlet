# Applications utility

`applications` (`org.scarlet-os.desktop.applications`) manages finished native
`.app` directories in `/applications`. It shares descriptor and executable
interpretation with stemd. Package inspection reads real files; the host build
can inspect explicit packages but disables live installation, removal and
catalog reconciliation. It does not present a simulated installed-app list.

## Operations and recovery

Inspect a source `<lowercase-slug>.app` before installing. The view displays its
name, stable descriptor ID, native target, file count and byte size. Inspection
checks structure and compatibility, not publisher identity or signatures.

Installation rechecks the inspected package, rejects an installed duplicate ID
or existing destination, copies into an ignored directory on the destination
filesystem, synchronizes files and verifies the copied tree. It then publishes
the app and calls stemd's `ReloadApplications`. A manager operation lock excludes
other Applications transactions. The native ABI exposes replacing rename;
publication checks for collisions and the filesystem rejects replacing a
nonempty app directory. This is not an atomic no-replace guarantee against an
unrelated process creating an empty directory during publication.

Removal requires confirmation of the captured app identity and rechecks that
package. The complete directory moves into an ignored recovery directory
before catalog reconciliation. Native removal retains the package because the
current ABI does not provide safe directory-relative recursive deletion. The
result displays the exact recovery path. The original `.app` filename is kept,
so inspecting and installing that path restores the package. Personal data
outside the package and already running processes are unaffected.

Legacy `/etc/stemd.d/apps` entries remain intact. Removing an app that overrides
a legacy entry with the same ID reveals that legacy launcher entry again;
removing the package does not necessarily remove its launcher identity.

If reconciliation fails, the filesystem change is rolled back and reconciliation
is attempted again. This second call also handles a transport timeout where the
first call may already have run. A second failure explicitly reports that the
catalog state is unconfirmed. Failed rollback or cleanup reports the retained
path. Nothing automatically deletes stale recovery directories or a lock left
after a crash. Closing is blocked while a transaction is active.

The supported Scarlet Rust toolchain must include native std revision
`26d00a8` or newer. The `scarlet-fs` library in `user/lib/scarlet-fs` supplies remaining
ABI capabilities such as no-follow opening, atomic exclusive file creation and
richer permission metadata. Native permission preservation is limited to the
reported read/write/execute capabilities; existing directory modes cannot be
changed. Unsupported preservation or cleanup produces a visible error.

## Validation bounds

A descriptor is limited to 1 MiB. Its root directory is streamed, retains only
the one descriptor entry, and stops after 100,000 entries. Duplicate descriptor
keys use a set and normalize whitespace consistently with the shared parser.
The complete package walk allows at most 100,000 items, 64 nesting levels and
8 GiB of regular-file data; copying independently enforces the same bounds.
The hashing buffer is allocated on the heap, so a deeply nested directory does
not add a 64 KiB stack buffer for every recursion level.

Finished trees reject symlinks, special files, nonportable relative filenames,
shipped build recipes, missing execute permission on the entry point, mixed ELF
architectures/ABIs, invalid program-header or segment bounds, missing loadable
segments, and interpreters other than `/bin/scarlet-ld`. The internal streaming
fingerprint detects ordinary source changes between inspection and copying; it
is not a cryptographic authenticity check. The current implementation reads
package contents when refreshing the managed list, so refresh work grows with
the installed bytes and runs on a worker thread.

## Evidence

Temporary-fixture tests cover the shared catalog rules, large/duplicate
descriptors, root and recursive bounds, successful staged installation,
collision/changed-source rejection, permissions, symlinks, embedded ELF files,
removal rollback, ambiguous reload failures and operation locking. No test
installs into the live `/applications` directory.

The descriptor review harness is preserved at
`user/lib/scarlet-apps/examples/descriptor_benchmark.rs` and validates the actual shared
production parser. Run it in the configured development shell with
`cargo run --release --manifest-path user/lib/scarlet-apps/Cargo.toml --example descriptor_benchmark`. After replacing linear duplicate-key scans with a set, one
optimized host run on 2026-10-22 measured:

| Unknown unique keys | Descriptor bytes | Validation time |
| ---: | ---: | ---: |
| 1,000 | 11,127 | 0.385 ms |
| 10,000 | 110,127 | 3.62 ms |
| 50,000 | 550,127 | 53.35 ms |

The review reported 4.36 s for the 50,000-key case before the change. These are
host observations under concurrent builds, not device performance guarantees.
The result remains valid and an additional late duplicate is rejected.

Host keyboard inspection, invalid-package feedback and window scrolling are
checked with an explicit scratch package. Native compilation uses the current
Scarlet toolchain. These checks do not demonstrate installation/removal in a
running Scarlet desktop or power-loss recovery.
