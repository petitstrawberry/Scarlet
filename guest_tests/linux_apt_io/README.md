# Linux APT / glibc I/O regression

This freestanding AArch64 Linux probe covers the descriptor limit used by
APT's exec helper, unsupported anonymous temporary-file fallback, and glibc's
`sendmmsg` DNS prerequisite. Datagram and seqpacket checks verify message
counts, `msg_len` output, record ordering, empty batches, and a failure after
one successful message. Pipe checks reproduce APT's select/write failure:
less than `PIPE_BUF` free space must not advertise writable, and a small
write across a page boundary must return `EAGAIN` without delivering a prefix.
The pipe write endpoint must advertise `O_WRONLY`, and seeking a pipe must
return `ESPIPE` so dpkg can discard tar padding by reading it instead.

The filesystem checks create a disposable directory on `/var/tmp` (ext2),
exercise dirfd-relative hardlinks, symlink-follow flags, root-only ownership,
cross-page chmod paths, and last-link unlink while a descriptor remains open.
Forked children check whole-file POSIX lock contention, `F_GETLK`'s owner PID,
release on closing another alias, release on exit, retention across a real
`execve` of `/usr/bin/sleep`, and release when exec closes a CLOEXEC alias.
The probe intentionally expects `EOPNOTSUPP` for byte ranges and a contended
sleeping lock request.

Build from the repository root:

```sh
direnv exec . clang --target=aarch64-unknown-linux-gnu \
  -ffreestanding -fno-stack-protector -fno-builtin -nostdlib -static \
  -fuse-ld=lld -Wl,-e,_start -Os \
  guest_tests/linux_apt_io/probe.c -o /tmp/apt-io-probe
```

Copy into a disposable guest, make it executable, and run from Linux Bash.
The probe removes `/var/tmp/scarlet-apt-probe` after a successful run; a failed
run may leave it behind, so use a fresh guest for subsequent checks.
All checks should print `PASS`, followed by `APT I/O regression PASSED`, with
exit status zero. Actual APT tests must additionally exercise DNS, signed
repository updates, dpkg installation, execution, and removal:

```sh
export LC_ALL=C DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get -y install hello jq
hello
jq -n '1 + 2'
dpkg --audit
apt-get -y remove hello
apt-get -y install hello
```

`exec_args.c` exercises the exec limit hit by Wine's postinst, which invokes
`update-alternatives` with 195 arguments. It checks complete argv/envp contents
in the new image, the 256-entry boundary, an 8 KiB argument, and `E2BIG`/`EFAULT`
failures. argv and envp currently allow 256 entries each and share a 128 KiB
string budget, including terminating NUL bytes.

```sh
direnv exec . clang --target=aarch64-unknown-linux-gnu \
  -ffreestanding -fno-stack-protector -fno-builtin -nostdlib -static \
  -fuse-ld=lld -Wl,-e,_start -Os \
  guest_tests/linux_apt_io/exec_args.c -o /tmp/exec-args-probe
```

Place this probe at `/var/tmp/exec-args-probe` in the guest (the executable
path is used for its child exec checks), and run it from Linux Bash. A passing
run ends with `Exec argument regression PASSED`. Additionally install the
Debian `wine` package, verify that its trigger finishes, and run `dpkg --audit`
and `update-alternatives --query wine`.
In the current x86-64 Wine bundle, use `/usr/local/bin/wine --version` to test
the Box64 launcher. Debian's `/usr/bin/wine` frontend directly execs the x86-64
`wine64` ELF, which the AArch64 Linux view cannot execute without that launcher.

`tty.c` checks that independently opened PTY slaves do not share `O_NONBLOCK`,
while duplicated descriptors and forked children do share it. It also checks
`F_GETFL` after aliases set and clear the flag, empty nonblocking `read`/`readv`,
zero-length reads, real canonical EOF, and blocking reads with input supplied
by the parent after a delay (including cross-page buffers).

```sh
direnv exec . clang --target=aarch64-unknown-linux-gnu \
  -ffreestanding -fno-stack-protector -fno-builtin -nostdlib -static \
  -fuse-ld=lld -Wl,-e,_start -Os \
  guest_tests/linux_apt_io/tty.c -o /tmp/tty-probe
```

Run in a disposable guest. A passing run ends with `Terminal regression PASSED`.
Also run `apt-get install nano` from interactive Linux Bash **without `-y`**.
Leave its confirmation prompt idle, then type `y` and Enter; it must wait for
that input and finish installation. An empty response must accept the default,
while `n` and actual stdin EOF must still abort.

The kernel child-ownership regression also models a registered child absent
from scheduler queues during dispatch. Linux wait must enumerate registered
process children rather than infer ownership from those transient queues;
otherwise APT can abandon a still-running dpkg with a false `ECHILD`.

The full-debian overlay supplies `99scarlet`: root acquisition workers,
APT's realloc-based package cache, and direct dpkg output without PTY master
ioctls. Repository signature checks retain their defaults. APT's root:adm
terminal-log ownership change currently warns because only root ownership is
supported. These checks do not establish compatibility with all maintainer
scripts, byte-range locks, or filesystem crash recovery. Unlinked ext2 inode
reclamation currently occurs before a later inode allocation after all live
nodes/open descriptions have been released.

Linux `statfs` still reports a fixed 1 GiB capacity and 768 MiB available,
independent of the actual filesystem counters. The tested image has an 8 GiB
ext2 root filesystem, but APT's capacity checks cannot use its real free space
yet; large installations may be incorrectly rejected and real low-space
conditions cannot be predicted from these counters.
