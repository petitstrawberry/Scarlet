# Linux descriptor access and exec regression

This freestanding AArch64 Linux program checks the real syscall paths for
read/write, positioned and vectored I/O, and duplication of close-on-exec
file descriptors followed by `execve`. It catches both unintended writes to
read-only executable files and descriptors that report FD_CLOEXEC cleared but
are nevertheless closed by exec (for example, Bash's restored standard output).

Build from the repository root with the development shell:

```sh
direnv exec . clang --target=aarch64-unknown-linux-gnu \
  -ffreestanding -fno-stack-protector -fno-builtin -nostdlib -static \
  -fuse-ld=lld -Wl,-e,_start -Os \
  guest_tests/linux_fd_access/probe.c -o /tmp/fd-access-probe
```

Copy that binary to `/tmp/fd-access-probe` in a disposable Scarlet guest, make
it executable, and run it from Linux Bash. That exact guest path is required
for its self-exec. It uses `/tmp/fd-access-probe.data` and
`/tmp/fd-access-probe.log`; avoid concurrent runs. No libc is needed.

Exit status zero and matching `actual` / `expected` hex values in the log mean
success. Both phases' failures affect the final exit status. The pre-exec
phase tests rejected I/O, unchanged file contents and duplication flags; the
post-exec phase checks inherited and closed descriptors and retained access
restrictions. On the old kernel, ordinary writes to O_RDONLY files succeeded
and both non-CLOEXEC duplicates disappeared at exec.
