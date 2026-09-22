# SQLite consumer acceptance test

This compiles **unmodified SQLite 3.53.4** and links an ordinary C `main` with
Scarlet's matching CRT and std-backed `libscarlet_c.a`. A separate, small
`scarlet-native` VFS implements SQLite's documented
[`SQLITE_OS_OTHER` interface](https://www.sqlite.org/c3ref/vfs.html) through real
Scarlet file descriptors, positioned I/O, filesystem sync and kernel locks.
This is an OS port of SQLite, not a claim that its Unix backend now runs or
that Scarlet implements all POSIX interfaces.

## Source and build

- [Official release archive](https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip).
- SHA-256: `1e71ddf93849c6a6ecf58b827c0692073d2dd7ee40196158068f7b29f422e87d`.
- SHA3-256, matching the [official download page](https://www.sqlite.org/download.html):
  `628a44cfe82c66aed1ccbbe85a562d2e33ebe64b3288981ed76285612227934e`.
- Source ID: `2026-07-24 19:02:57 bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc`.
- SQLite is [public domain](https://www.sqlite.org/copyright.html). Its source
  headers are retained unchanged in the verified, extracted archive.

```sh
python3 tools/native-rustc/consumer-sqlite/build.py \
  --target aarch64-unknown-scarlet \
  --sysroot /path/to/scarlet-cross-sysroot \
  --libc /path/to/aarch64-unknown-scarlet/release/libscarlet_c.a \
  --clang /path/to/unwrapped/clang \
  --ar /path/to/llvm-ar \
  --linker /path/to/ld.lld \
  --output /tmp/scarlet-sqlite-aarch64
```

`--source-archive /path/to/sqlite-amalgamation-3530400.zip` reuses a local copy
only if both pinned hashes match. The output directory must not exist. RV64
uses `--target riscv64gc-unknown-scarlet` with matching inputs. The builder
checks archive members before extraction, snapshots Scarlet headers and both
fixture sources, and records source/header/archive/CRT/binary hashes, compiler
commands and versions, archive architecture checks and the final static ELF
audit. A compiled binary is explicitly reported as not yet executed on Scarlet.

Only Scarlet headers and Clang builtin headers are used, with `-nostdinc`,
`-ffreestanding`, `-fno-builtin`, and no host configuration or source patches.
The upstream compilation disables unused-parameter warnings because its mmap
stubs intentionally have unused arguments; fixture and VFS warnings remain
errors. The configuration is explicit:

```text
NDEBUG SQLITE_OS_OTHER=1 SQLITE_THREADSAFE=0
SQLITE_OMIT_LOAD_EXTENSION SQLITE_OMIT_LOCALTIME SQLITE_TEMP_STORE=3
SQLITE_OMIT_WAL SQLITE_MAX_MMAP_SIZE=0
```

SQL floating point, prepared statements, transactions, rollback journals,
UTF-8, indexes, joins, BLOBs and the usual built-in SQL functions remain enabled.
SQLite uses its own numeric formatter, independently of libc printf support.

## Guest acceptance

Create a new writable directory first, then run these as **separate processes**:

```sh
/system/bin/sqlite-probe /tmp/sqlite-consumer create
/system/bin/sqlite-probe /tmp/sqlite-consumer verify
/system/bin/sqlite-probe /tmp/sqlite-consumer crash
/system/bin/sqlite-probe /tmp/sqlite-consumer verify
```

`create` and both `verify` runs must return **53** and print the exact line
`SCARLET_LIBC_SQLITE_OK`. The deliberate `crash` run must print
`SCARLET_LIBC_SQLITE_CRASH_READY`, flush stdout, and terminate via libc `abort`
with Scarlet status **134**. It must not print the ordinary success marker.
The next `verify` process must recover the hot journal and validate the original
committed rows. The native-rustc harness runs the sequence on both ext2 and
tmpfs, then independently validates the extracted ext2 database with the host's
SQLite implementation after the VM stops.

The fixture covers:

- Direct VFS I/O larger than the Native 64 KiB transfer limit, partial-EOF and
  beyond-EOF zero filling, file size, truncation, sync, exclusive creation,
  unknown file controls, and deletion with parent-directory sync.
- Actual kernel lock contention between separately opened file descriptions,
  logical SQLite lock upgrades/downgrades and release on close.
- A 131,113-byte deterministic BLOB, including incremental BLOB read/write
  across offset 65,539, plus exact byte comparison after reopen.
- Prepared/bound inserts, UTF-8 labels, REAL values, indexed joins, grouping,
  transaction commit/rollback, savepoint rollback, VACUUM, time/random SQL,
  `integrity_check`, read-only write rejection, and two-connection BUSY/retry.
- Process-crash rollback: a small cache and `sqlite3_db_cacheflush` force
  uncommitted pages into the database. The fixture compares raw database bytes
  against a pre-transaction snapshot and checks the rollback journal's magic
  and size before aborting, so an unspilled in-memory transaction cannot pass.

The final database is `sqlite-roundtrip.db`, with 24 rows in `items` and three
in `groups`. The SQL schema and deterministic data are visible in `main.c`.
Successful create/verify leaves the database for independent inspection and no
active rollback journal. The crash intermediate intentionally leaves one.

## VFS limits

The file methods use version 1: there is no WAL, shared-memory mapping, mmap,
disk temporary-file implementation or dynamic extension loading. VFS version
2 is used only for the integer clock callback. `SQLITE_TEMP_STORE=3` keeps
SQLite's temporary databases and subjournals in memory. Calls requesting
unsupported disk file types fail instead of pretending to open them.

Every SHARED-or-higher SQLite lock holds a **real exclusive, nonblocking kernel
flock**, retained until SQLite unlocks to NONE or the file closes. Logical lock
upgrades do not bypass another owner's physical lock. This deliberately
serializes readers as well as writers, following the same conservative pattern
as SQLite's upstream flock VFS. Locks are advisory: unrelated applications
that directly overwrite a database can still corrupt it. `SQLITE_THREADSAFE=0`
requires one thread to use this SQLite library at a time, including distinct
connections; separate processes are protected by the kernel file locks.

Pathnames are bounded by Scarlet's 1024-byte native limit. Existing symlink
aliases are canonicalized, and a new leaf uses its real parent plus basename.
The final native open uses `O_NOFOLLOW` to reject dangling or changed leaf
symlinks. A caller's explicit `SQLITE_OPEN_NOFOLLOW` request is rejected because
this VFS cannot establish that no original path component was a symlink after
canonicalization. Renaming or hard-linking a live database is outside this
acceptance scope; do not use different journal paths for the same live inode.
The access callback uses real opens, so it is not a complete POSIX `access()`
implementation; an unreadable possibly existing journal is an I/O error, not
silently considered absent.

Journal sync flushes the file then its parent directory, and deletion honors
SQLite's requested parent sync. Sync errors propagate. Device capability flags
are zero: no atomic-write, powersafe-overwrite or sector-atomic guarantees are
invented. Current Scarlet sync and block-device behavior has not established
power-loss durability. Passing process-crash recovery and persisted ext2 tests
is not proof of that property or an exhaustive SQLite/libc conformance test.
