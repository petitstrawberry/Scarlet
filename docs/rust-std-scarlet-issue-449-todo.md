# Rust std on Scarlet Native: Issue #449 TODO

Canonical tracker: <https://github.com/petitstrawberry/Scarlet/issues/449>

This document is the local execution checklist for issue #449. The issue
milestones are sequential: do not treat later smoke-test progress as completion
evidence for an earlier extraction or API-boundary milestone.

## Current Position

Current phase: **M7 userland process bring-up**, with kernel-facing gaps split
into separate follow-up issues before deeper API work.

M0, M1, and M2 are treated as the baseline for current work:

- M0 extracted the Scarlet low-level crate boundaries.
- M1 added the Rust target skeleton.
- M2 made stdout/stderr, exit, allocator, and `println!` usable enough for
  Scarlet `std` smoke programs.

M3 has an AArch64 QEMU smoke pass for a normal Rust `std` program covering file,
I/O, argument, environment, current-directory, directory iteration, and hardlink
behavior. M5 and M6 now also have AArch64 QEMU smoke passes for basic localhost
networking and thread spawn/join behavior. M7 now has an AArch64 QEMU smoke
pass for `Command::spawn`, `Command::status`, `Child::wait`,
`Child::try_wait`, exit-code preservation, environment propagation, child
`current_dir`, stdin/stdout/stderr pipes, `Command::output()`,
`Stdio::null()`, file stdio, and parent stdout/stderr redirection.

## Milestone State

### M0: Extract Scarlet low-level libraries from `scarlet_std`

Status: baseline complete, with follow-up cleanup allowed.

- `scarlet-abi`: ABI constants, syscall numbers, raw ABI structs and IDs.
- `scarlet-sys`: unsafe syscall bindings and arch syscall asm.
- `scarlet-rt`: runtime glue split out for legacy no_std and future Rust std
  reuse.
- `scarlet-os`: safe-ish Scarlet-specific OS wrappers.
- `scarlet-std`: compatibility facade for existing userland code.

Follow-up cleanup:

- Keep moving direct raw-syscall logic out of `scarlet-std` where it belongs in
  `scarlet-os` or `scarlet-sys`.
- Keep Rust crate imports underscore-based, but Cargo package names hyphenated
  (`scarlet-abi`, `scarlet-sys`, `scarlet-rt`, `scarlet-os`, `scarlet-std`).

### M1: Rust target skeleton

Status: baseline complete.

- Scarlet Native targets exist in the Rust fork.
- `target_os = "scarlet"` routes to the Scarlet PAL.
- Scarlet ABI/OSABI handling is wired into the target path.

Follow-up cleanup:

- Keep target changes minimal and Scarlet-specific.
- Do not assume upstream Rust target acceptance in this issue.

### M2: `println!` / allocator / exit

Status: baseline complete.

- `stdout` / `stderr` write through Scarlet Native stream handles.
- `exit_group` is wired.
- allocator support is sufficient for smoke programs.
- `println!` / `eprintln!` smoke programs run on Scarlet.

Follow-up cleanup:

- Keep allocator glue isolated so it does not fight Rust fork integration.
- Do not move Scarlet-specific object APIs into Rust `std`.

### M3: `std::fs`, `std::io`, `std::env`

Status: smoke-validated on AArch64; follow-up cleanup remains.

Implemented or partially implemented:

- `File::open`, `read`, `write`, `seek`, `close`.
- `metadata` / `stat` / `File::size`.
- `read_dir` through directory stream reads.
- `std::env::args`.
- `std::env::{vars, var, set_var, remove_var}` as process-local runtime state.
- `std::env::{current_dir, set_current_dir}` through VFS cwd syscalls.
- `std::io::pipe()` through Native `Pipe` and stream handles.
- `std::fs::hard_link` through Native `VfsCreateHardlink` where the filesystem
  implements hard links.

M3 TODO:

- Keep the M3 smoke program available and extend it when regressions are found.
  The current smoke covers `args`, `vars`, `current_dir`, `set_current_dir`,
  `File`, `read_to_string`, `write`, `metadata`, `read_dir`, `rename`,
  `remove_file`, and supported hardlink behavior on TmpFS.
- Keep verifying directory iteration on at least TmpFS and root/ext2. If a filesystem
  returns entries in an incompatible format, fix the kernel stream entry ABI or
  add a TODO stub only when the kernel genuinely lacks the feature.
- Verify pipe/tty/devfs behavior through std I/O where the kernel already
  exposes a compatible stream handle. `std::io::pipe()` is connected; process
  stdout piping is connected through M7 `std::process::Command`.
- Decide the compatibility policy for old `scarlet_std::{fs,io,env}`:
  keep as facade, deprecate later, and avoid expanding it with APIs that should
  be Rust `std`.
- Keep unsupported functions as explicit `TODO(scarlet): ...` stubs:
  advisory file locks, chmod-style permission mutation, timestamp setters,
  `current_exe`, and clocks until Native primitives exist.

Exit criteria:

- `./x check library/std` for AArch64 and RISC-V Scarlet targets. AArch64 is
  passing; RISC-V still needs the same verification run.
- `./x build library/std --target aarch64-unknown-scarlet` is passing.
- A QEMU M3 smoke run on AArch64 is passing.
- No direct regression in userland `scarlet-abi`, `scarlet-sys`, `scarlet-rt`,
  `scarlet-os`, or kernel checks touched by the work.

### M4: Native socket API stabilization for `std::net`

Status: partially implemented.

Implemented or partially implemented:

- Native socket syscall numbers already exist in `scarlet-abi` / `scarlet-sys`.
- Rust std PAL has thin wrappers for create, bind, connect, listen, accept,
  shutdown, sendto, and recvfrom.
- `std::net` stores Scarlet Native socket handles directly and closes/duplicates
  them through Native handle operations.
- IPv4 address conversion is wired for both the Native `Inet4SocketAddress`
  record and the compact 8-byte datagram sockaddr used by sendto/recvfrom.

TODO:

- Define and expose `getsockname` / `getpeername`-equivalent Native operations
  before claiming complete peer/local address reporting. Accepted TCP peer
  address tracking is split out as <https://github.com/petitstrawberry/Scarlet/issues/453>.
- Define blocking, nonblocking, timeout, and option semantics. These need
  Native socket API support before Rust `std` can do more than explicit
  `Unsupported` errors.
- Define IPv6 address structs and conversions before claiming IPv6 support.
- Keep DNS resolver out of the critical path unless the Native API is ready.

### M5: `std::net` basic support

Status: basic smoke-validated on AArch64; API hardening remains.

Implemented or partially implemented:

- `TcpStream::connect` for IPv4 socket addresses.
- `TcpStream` `Read` / `Write` through Native stream handles.
- `TcpListener::{bind, accept}` for IPv4. Accepted peer address is currently
  `0.0.0.0:0` until the Native accept path returns the peer.
- `UdpSocket::{bind, send_to, recv_from}` for IPv4.
- Connected UDP `send` / `recv` through Native stream handles after
  `UdpSocket::connect`.
- Connected UDP `peer_addr()` from the std PAL's userland connection cache.
- IP literal lookup for IPv4.
- AArch64 QEMU M5 smoke covering IPv4 literal parsing, UDP loopback
  send/receive, TCP loopback accept/read/write, and connected UDP peer address.

TODO:

- Replace the temporary kernel localhost direct path with proper `lo`
  interface/routing support: <https://github.com/petitstrawberry/Scarlet/issues/452>.
- Make `TcpListener::accept()` return the real accepted peer address:
  <https://github.com/petitstrawberry/Scarlet/issues/453>.
- Implement or keep explicit `Unsupported` errors for `peek`, timeout,
  nonblocking, TTL, linger, multicast, broadcast, and socket error state until
  the Native API is settled.
- DNS support after resolver policy is decided.

### M6: thread / sync / time

Status: basic smoke-validated on AArch64; runtime hardening remains.

Implemented or partially implemented:

- `thread::yield_now` through Native `Yield`.
- `thread::sleep` through Native `Sleep` in nanoseconds.
- `std::thread::spawn` / `join` through Native `Clone`, `Waitpid`,
  `ThreadDetach`, and `ThreadExitCleanup`.
- `thread::available_parallelism` returns `1` until the kernel exposes online
  CPU count.
- AArch64 QEMU M6 smoke covering `available_parallelism`, spawn/join,
  `yield_now`, `sleep`, shared atomics, and detached thread execution.

TODO:

- Extend the M6 smoke program with panic propagation through `JoinHandle`,
  scoped thread basics, and TLS destructor behavior under QEMU.
- Audit the TLS model and runtime handoff against Rust `std`'s expectations,
  especially after the Rust fork starts using compiled TLS sections rather than
  the Scarlet runtime's manually mapped TLS page.
- Native OS thread id reporting.
- `Instant` / `SystemTime`.
- `Mutex` / `Condvar` / `RwLock` backend.
- Add futex-like wait/wake syscall if the existing kernel primitives are not
  enough.

### M7: process / pipe / Command

Status: M7 process spawn/status and stdio redirection smoke-validated on
AArch64; `Child::kill()` remains blocked on a Native kernel implementation.

Implemented or partially implemented:

- `target_os = "scarlet"` now routes `std::process` to a Scarlet backend.
- `Command::spawn()` uses Native `Clone` with non-shared VM/FS/FILES semantics,
  then executes the child with Native `Execve`.
- `Command::status()` works through `spawn()` plus `Child::wait()`.
- `Child::wait()` and `Child::try_wait()` use Native `Waitpid`.
- `ExitStatus` preserves the Native process exit code.
- `argv`, captured environment, explicit environment overrides/removals, and
  child `current_dir()` are prepared before clone and passed to `execve`.
- Minimal PATH search is implemented in the std PAL before `execve`.
- `Stdio::piped()` creates Scarlet Native pipes and remaps child stdio with the
  same close-then-duplicate handle pattern used by legacy Scarlet userland.
- `From<ChildPipe> for Stdio` is wired for reusing a child pipe as child stdio.
- `Stdio::null()` opens `/dev/null` with the stream-appropriate direction and
  remaps it into child stdio.
- `Stdio::from(File)` duplicates the file handle and remaps it into child
  stdio.
- `Stdio::from(io::Stdout)` and `Stdio::from(io::Stderr)` duplicate parent
  handles before clone-side stdio remapping, so cross-stream cases avoid
  sequential remap clobbering.
- `Command::output()` captures stdout and stderr through Native pipes and drains
  both concurrently with Scarlet std threads.
- The Rust/LLVM fork emits Scarlet ELF OSABI directly; no post-link
  `set_osabi.py` step is required for the smoke binary.
- M7 smoke source exists at `rust/scarlet-smoke/m7-std-process.rs`.
- AArch64 QEMU M7 smoke passes when linked with the Rust fork std and installed
  as `/bin/scarlet-rust-std-m7-smoke`; it covers child status, explicit failure
  exit code 23, spawn plus try-wait/wait, env propagation, child `current_dir`,
  stdin pipe writes, stdout/stderr pipe reads, `Command::output()`,
  `Stdio::null()`, `Stdio::from(File)`, and parent stdout/stderr redirection.

TODO:

- `Child::kill()` after Native syscall 6 is wired to a kernel implementation:
  <https://github.com/petitstrawberry/Scarlet/issues/454>.
- Decide how `execve` errors should be reported to the parent. The current M7a
  implementation returns a spawned child and exits with 127 if child-side
  `chdir` or `execve` fails after clone.
- Keep Cross-ABI process control in `scarlet-os`, not Rust `std`.

### M8: Scarlet crates migration

Status: blocked on usable M3/M6/M7 surface.

TODO:

- Audit `scarlet-std` / userlib consumers for APIs replaceable by Rust `std`.
- Try SWS, ScarletUI, IME clients, and related native crates on Rust `std`.
- Decide no_std compatibility and deprecation paths.
- Document recommended crate names for Scarlet-specific app features.

## Implementation Rules

- Work milestones in order unless a later investigation is needed to unblock the
  current milestone.
- When the kernel already has a primitive, connect Rust `std` to it rather than
  stubbing.
- When the kernel lacks a primitive, return `unsupported()` and leave a precise
  `TODO(scarlet): ...` comment naming the missing Native primitive.
- Do not route Scarlet-specific capabilities through Rust `std`; keep them in
  `scarlet-os` or protocol crates.
- Runtime proof matters: each milestone after M2 needs a QEMU smoke program, not
  only Rust fork build success.

## `scarlet-std` Compatibility Policy

`scarlet-std` is the compatibility facade for existing no_std Scarlet userland.
Do not add new std-shaped APIs there just because Rust `std` needs them. New API
placement should follow this split:

- Raw syscall numbers, fixed ABI records, constants: `scarlet-abi`.
- Unsafe syscall wrappers and arch syscall asm: `scarlet-sys`.
- Runtime entry, allocator, panic/abort, argc/argv/env handoff: `scarlet-rt`.
- Scarlet-specific safe wrappers such as handles, IPC, sockets, hypervisor,
  native task/object controls: `scarlet-os`.
- Portable Rust application APIs such as `std::fs`, `std::io`, `std::env`,
  `std::net`, `std::thread`, `std::process`: Rust fork `library/std`.
- Existing `scarlet_std::{fs, io, env}` remains for compatibility and should
  either delegate to lower-level crates or stay unchanged until deprecation.
