# Linux tty read regression (AArch64)

Build this freestanding Linux program from the repository root:

```sh
direnv exec . clang --target=aarch64-unknown-linux-gnu \
  -ffreestanding -fno-stack-protector -fno-builtin -nostdlib -static \
  -fuse-ld=lld -Wl,-e,_start -Os \
  guest_tests/linux_tty_read/probe.c -o /tmp/tty-read-probe
```

Copy it to a disposable Scarlet guest and run it from Linux Bash on an idle
terminal, with no queued input. It duplicates stdin, changes O_NONBLOCK through
the original descriptor, and reads through the duplicate using single-page and
cross-page buffers. Both reads must return -EAGAIN, not EOF or -EPERM. It restores
the original descriptor flags and exits zero on success. Do not pipe input into
this test: stdin must be a tty.

This reproduces Wine conhost interpreting an empty terminal as end-of-file and
closing an interactive cmd immediately after displaying its prompt.
