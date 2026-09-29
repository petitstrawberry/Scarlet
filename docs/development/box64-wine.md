# Box64 and Wine on the AArch64 Linux ABI

The `full-debian` image uses the `wine` profile from
[scarlet-bundle-debian](https://github.com/petitstrawberry/scarlet-bundle-debian).
It contains native AArch64 Box64 and Debian's amd64 Wine64 packages in the
`linux-aarch64` view. The initial target is 64-bit Windows console programs.
Wine32/Box86/WoW64, GUI, audio and GPU integration are outside this first
bring-up. Native Linux CI success does not establish Scarlet ABI compatibility.
The v0.2.0 Linux console smoke passes and exits normally. Multimedia
initialization still reports unresolved OpenCL, OpenMP and Zstd wrapper symbols;
media playback and full native-library coverage remain open work.

The producer builds and checks the archive in GitHub Actions. Build and boot
the Scarlet image using the existing `aarch64-limine-full` project workflow.

## Probes

Run these individually, in order. `abi-run` opens an ELF image directly, so
invoke the Wine shell launchers through `/bin/sh`:

```sh
abi-run linux-aarch64 /usr/local/bin/box64 --version
abi-run linux-aarch64 /bin/sh /usr/local/bin/wine --version
abi-run linux-aarch64 /bin/sh /usr/local/bin/wineserver --version
abi-run linux-aarch64 /usr/bin/env WINEDLLOVERRIDES=mscoree,mshtml= /bin/sh /usr/local/bin/wine cmd /c ver
```

The first command tests native Box64 startup. The next two execute x86-64 Linux
programs through Box64. The final command initializes `~/.wine` on first use and
exercises Windows PE loading and Wine's server/process/thread/IPC paths. The
override disables Mono/Gecko loading and download prompts for this probe.

Then try a 64-bit Windows executable placed in the shared directory:

```sh
abi-run linux-aarch64 /bin/sh /usr/local/bin/wine /shared/hello.exe
```

For a separate test prefix, add `WINEPREFIX=/tmp/scarlet-wine-test` after
`/usr/bin/env`. Upstream's prefix-specific stop command is:

```sh
abi-run linux-aarch64 /usr/bin/env WINEPREFIX=/tmp/scarlet-wine-test /bin/sh /usr/local/bin/wineserver -k
```

This currently fails on Scarlet: Linux `fcntl(F_GETLK)` is unimplemented, so
Wine cannot obtain the lock owner's PID. See the cancellation procedure below.

## Diagnosing a failure

First record which of the probes fails and its complete output. To compare
with the Box64 interpreter, use:

```sh
abi-run linux-aarch64 /usr/bin/env BOX64_DYNAREC=0 BOX64_LOG=1 WINEDLLOVERRIDES=mscoree,mshtml= /bin/sh /usr/local/bin/wine cmd /c ver
```

The ordinary launch uses ARM64 dynarec. The interpreter comparison can help
separate generated-code issues from shared loader, syscall and library paths;
it does not by itself identify the missing ABI behavior.

The launchers call `/usr/local/bin/box64` explicitly with
`/usr/lib/wine/wine64` or `/usr/lib/wine/wineserver64`. No x86-64 kernel ABI or
binfmt_misc registration is needed. Dependencies of both architectures remain
managed by Debian's dpkg database.
Wine's adjacent `/usr/lib/wine/wineserver` selector also points to the Box64
launcher. The original Debian script is preserved as `wineserver.debian` with
a dpkg diversion; this prevents its native shell from directly executing an
amd64 ELF outside Box64.

### Prefix capacity and slow initialization

The AArch64 full project reserves a minimum 8 GiB rootfs. Automatic sizing of
the distribution alone previously left about 424 MiB free, while a Wine prefix
observed during initialization already occupied 770 MiB. `copy error 112` is
`ERROR_DISK_FULL`; subsequent missing-path and registration errors can be
consequences of the incomplete prefix. Scarlet's Linux `statfs` currently
returns fixed compatibility capacity values, so guest `df` does not establish
the actual free space on ext2.

Changing `min-size-mib` and composing images regenerates the filesystem from
its layers; it does not grow an existing guest filesystem in place. After a
capacity failure, use a fresh test prefix on an adequately sized image rather
than treating a partially populated prefix as a valid installation.

Large prefixes also exposed a kernel page-cache cost: closing each file scanned
every cached file's pages to check for dirty data, including during `execve`
and process exit. These operations now use the requested file's ordered key
range. This reduces work and time spent holding the cache's IRQ lock.

### Startup after prefix creation

An initialized prefix still needs Wine services to start when no wineserver is
running. The bundled wineserver launcher now keeps the server and services alive
for 60 seconds after the last user process exits. Starting another application
within that window reuses them. Previously `-p0` scheduled shutdown immediately,
so repeating a short command repeated service startup even when the prefix and
filesystem cache were warm.
Wine's [server shutdown implementation](https://github.com/wine-mirror/wine/blob/wine-10.0/server/process.c#L597)
uses the persistence timeout to decide when to shut down those services.

On the rebuilt image under HVF with 8 vCPUs and 16 GiB RAM, successful
command-to-return measurements with an initialized prefix and `WINEDEBUG=-all`
were:

| Invocation and server state | Elapsed |
| --- | ---: |
| `wine cmd /c ver`, retention disabled, server stopped | 17.56 s |
| `wine cmd /c ver`, default retained server | 5.27–5.50 s |
| `wine 'C:\windows\system32\cmd.exe' /c ver`, retained server | 3.16–3.40 s |

The retained-server invocations all returned zero and printed the version.
Fresh prefix creation took 185.64 seconds but returned 137 after printing the
version; a second retention-disabled invocation also returned 137. Those are
failures, not successful startup timings. Some helper processes still exited
139/137, so the improvements do not establish full Wine service compatibility.

These are observations on Scarlet, not a native-Linux performance baseline.
Earlier service traces showed work spread across startup, rather than one dominating
10/30-second service timeout. The `environ` trace also showed `wine cmd` loading
`start.exe /exec cmd /c ver`; the explicit Windows path avoids that extra PE
process. DLL and conhost initialization still consume time with services alive.
Ten `ver` commands in one explicitly addressed cmd process took 3.34 seconds
total, consistent with startup dominating this small workload.

The default retention applies to ordinary Wine invocations; no separate server
launch is needed. In Linux Bash, for an initialized prefix:

```sh
export WINEPREFIX=/root/.wine
/usr/local/bin/wine 'C:\windows\system32\cmd.exe' /c ver
```

Repeat the last command within 60 seconds. The first command execution still
starts Wine's services; later executions reuse them. This retains the server
and its service processes, including their memory, until the idle timeout.
Use the same prefix throughout.

Set `SCARLET_WINESERVER_IDLE_SECONDS` before the server starts to choose another
nonnegative integer duration. Zero restores immediate shutdown:

```sh
SCARLET_WINESERVER_IDLE_SECONDS=0 /usr/local/bin/wine cmd /c ver
```

An explicit wineserver `-p` option takes precedence over this default. Changing
the environment does not reconfigure a server that is already running. Starting
another server over an existing one is not a validated way to change persistence
on Scarlet because POSIX record locking is incomplete (see below).

### Cancelling bootstrap

Ctrl-C during bootstrap still does not immediately cancel startup on the new
image. Wine 10's
[server_init_process](https://github.com/wine-mirror/wine/blob/wine-10.0/dlls/ntdll/unix/server.c#L1571)
blocks SIGINT before
[waiting for wineboot](https://github.com/wine-mirror/wine/blob/wine-10.0/dlls/ntdll/unix/env.c#L1594).
The initial thread only
[unblocks it later](https://github.com/wine-mirror/wine/blob/wine-10.0/dlls/ntdll/unix/signal_x86_64.c#L2669).
An observed fresh-prefix run returned after 207 seconds: the pending SIGINT
entered its handler after wineboot reported completion. Prefix files continued
growing while the console was quiet, and SSH remained responsive. The shell's
zero exit status after this cancellation is not a successful `cmd` smoke test.

For a controlled run with exactly one Wine server, use a second Scarlet native
terminal or SSH connection to list tasks:

```sh
ps -l
```

Identify the PID whose command is `wineserver64`, then send it SIGINT (replace
`SERVER_PID` with that numeric PID):

```sh
kill -2 SERVER_PID
```

This stopped the tested fresh-prefix bootstrap and returned the original Bash
prompt within the next five-second capture. It stops every Wine process using
that server and can leave an incomplete prefix. If multiple servers are running,
identify the intended one before signalling it; `ps -l` alone does not show the
prefix. Do not interpret `wineserver -k` or `wineserver -w` as reliable lifecycle
controls until Linux POSIX record locks are implemented: `F_SETLK`/`F_SETLKW`
currently accept requests without tracking locks, while `F_GETLK` returns
`ENOSYS`. Wine's
[kill_lock_owner](https://github.com/wine-mirror/wine/blob/wine-10.0/server/request.c#L717)
depends on that lookup.

There was also an exit-status bug: wineserver's delayed SIGKILL could overwrite
the recorded cause of death of an already-exited, unreaped process. A normal
exit then appeared as status 137 in Bash. Remote signal delivery now leaves
zombie/terminated tasks unchanged, and fatal-signal delivery is serialized with
the start of kernel exit cleanup. Some initialized-prefix `cmd /c ver` probes
exit zero, but this does not resolve every status 137 (see below). First-time
initialization and unsupported service startup can still take substantial time.
Serial-console Ctrl-C was observed
entering Wine's control handler after initialization; GUI terminal behavior
has not been verified by that probe.

### Interactive prompt immediately exits

The September 30 follow-up reproduced `cmd` showing `Z:\>` and then returning
`Killed` on a fresh image. Wine's conhost trace showed its tty input read ending
with `STATUS_END_OF_FILE` even though the terminal was open and idle. The Linux
`read` path split a cross-page buffer into separate stream reads and turned
`WouldBlock` into zero bytes when its local descriptor flags indicated blocking
mode. Duplicated/inherited tty handles could already be nonblocking while those
local flags were stale. The single-page path instead incorrectly returned EPERM.

Linux `read` now respects the shared object's nonblocking state, preserves
EAGAIN, retries blocking reads after readiness, and treats a cross-page buffer
as one stream read. A freestanding reproducer in `guest_tests/linux_tty_read/`
checks the real single-page and cross-page syscall paths. With the release
kernel on HVF/SMP-8, interactive `cmd` remained waiting for over a minute and
accepted repeated `ver` commands; the old kernel closed the prompt without input.
All 27 targeted kernel read/signal/exit tests and the existing descriptor-access
guest regression passed.

Explicit `exit /b 7` still sometimes produces Unix status 137. In the follow-up
trace, Wine recorded Windows exit code 7 and Box64 reached the wrapped `exit(7)`;
wineserver subsequently sent SIGKILL before the Unix process finished. A sampled
client was still executing userspace code, not kernel exit cleanup. This is a
separate unresolved teardown issue, not evidence that the tty EOF fix failed or
that the guest ran out of memory. Wine's
[process cleanup timer](https://github.com/wine-mirror/wine/blob/wine-10.0/server/process.c)
does send SIGKILL to lingering Unix processes; the exact reason Box64 does not
finish in time remains unconfirmed. Do not suppress legitimate SIGKILL or
rewrite status 137 to zero as a workaround.

### Restored standard output and executable corruption

Repeated startup testing exposed two Linux ABI bugs together. `dup` and `dup3`
cleared the Linux-visible close-on-exec flag but retained it in native handle
metadata. When Bash restored stdout from a saved CLOEXEC descriptor, the next
exec could lose stdout. Box64 could then open an ELF using that free descriptor.
The ordinary and vectored I/O paths also allowed writes through read-only file
handles. In one test VM, 143 bytes at offset `0x2000` in `wineserver64` were
replaced by a Box64 mmap-warning log, causing later starts to crash.

Duplication now synchronizes the close-on-exec metadata, and the read/write,
positioned and vectored I/O paths validate the handle's access mode. The
freestanding regression in `guest_tests/linux_fd_access/` checks rejected I/O,
file contents, and descriptor survival across a real exec. These fixes prevent
this corruption path; they do not repair binaries already damaged in an old
guest image. Rebuild the image or restore affected files from a clean bundle.

## Sources and notices

The producer publishes the binary archive together with exact Debian source
packages for both architectures, package notices and its build scripts. The
Box64 build is commit/hash pinned; `/usr/share/doc/box64/` includes its license,
build provenance and build input source archive with per-file notices.
Upstream's prebuilt library and bash/test binaries are excluded. See the
producer's `ATTRIBUTION.md` and release assets for the corresponding sources.
