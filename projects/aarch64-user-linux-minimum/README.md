# AArch64 Linux hosted Scarlet, minimum project

This BSP boots Scarlet as a host process on AArch64 Linux with 4096-byte pages.
It uses the shared hosted runtime to execute existing guest ELFs in AArch64
address-space workers. The project provides a writable ext2 root and a host
terminal connected to Scarlet's TTY for boot and ELF execution validation.

From the repository's [Nix development environment](../../docs/development/README.md#development-environment)
on the Linux host:

```sh
getconf PAGE_SIZE  # 4096
cargo scarlet run --project projects/aarch64-user-linux-minimum --release --locked
```

The SDK builds the BSP, then composes the project's own newc initramfs and ext2
root under `.scarlet/images/`. The root filesystem uses the `base` and
`cli-utils` bundles. The initramfs boots the regular Scarlet `/init`.
It mounts `/dev/hostroot` and starts the base distribution's `stemd`, which
starts the login service on `/dev/tty0`.

Log in as `root` with an empty password at the local terminal, then run
`/bin/hello`. It is the base bundle's existing AArch64 Scarlet ELF and uses the
installed `/bin/scarlet-ld` interpreter. Expect `Hello, world!` and
`file = scarlet rust std` before the shell prompt returns. Press **Ctrl-]** followed
by **q** to stop the host instance and restore the terminal settings.

To build the artifacts and run the automated version of the same test:

```sh
cargo scarlet image --project projects/aarch64-user-linux-minimum --release --locked
python3 projects/aarch64-user-linux-minimum/tools/smoke_elf.py
```

The smoke test opens a PTY and invokes `cargo scarlet run --no-image` with the
project's release artifacts. Subsequent interactive boots can also use
`--no-image` to reuse the writable root image.
