# RV32GC SBI desktop

Scarlet's desktop bundle on QEMU's 32-bit RISC-V `virt` machine. The project
boots through OpenSBI, loads base and CLI tools from an initramfs, and mounts
an ext2 root filesystem containing the desktop, services, and applications.

The kernel uses `riscv32ima-unknown-none-elf` with `linux-boot`, `network`,
and `user-fpu`; native userspace uses `riscv32gc-unknown-scarlet` (`ilp32d`).
The BSP links the in-tree kernel with `kernel/lds/riscv32_sbi.ld`.

## Build and run

The development environment must provide a `cargo-scarlet` with RV32 target
selection and a Scarlet Rust toolchain containing RV32GC `std`. These are
provided by `scarlet-sdk` and `scarlet-rust-nix`, respectively.

Run from the repository root:

```sh
nix develop
cargo make image-riscv32
cargo make run-riscv32
```

`build-riscv32` builds the debug kernel/BSP only. `run-debug-riscv32` selects
debug images; `debug-riscv32` also pauses QEMU with a GDB server on port 12345.

The default boot command line is
`console=ttyS0 root=/dev/vblk0 rootfstype=ext2`. Rootfs contents persist in
`.scarlet/images/rootfs-riscv32-sbi-desktop.ext2`; image regeneration can replace
that file. Use `SCARLET_QEMU_SNAPSHOT=1` for disposable guest changes.

## Devices and runner settings

The runner defaults to four harts, 768 MiB of RAM, and TCG. GPU, block storage,
network, keyboard, mouse, and RNG use VirtIO MMIO transports. This avoids
depending on firmware-assigned PCI BARs and a large PCI ECAM mapping under
Sv32. The display uses the same host selection as the full projects: Cocoa
on macOS, GTK/SDL on a Linux desktop, and VNC on a headless host.

| Variable | Default |
| --- | --- |
| `SCARLET_QEMU_BINARY` | `qemu-system-riscv32` |
| `SCARLET_QEMU_CPU_RV32` | `rv32` |
| `SCARLET_QEMU_BIOS_RV32` | `default` (bundled OpenSBI) |
| `SCARLET_QEMU_SMP` | `4` |
| `SCARLET_QEMU_MEMORY` | `768M` |
| `SCARLET_QEMU_DISPLAY` | Host-selected display |
| `SCARLET_QEMU_GPU` | VirtIO GPU, with GL for GL displays |
| `SCARLET_QEMU_SERIAL` | `mon:stdio` |
| `SCARLET_QEMU_NET` / `SCARLET_QEMU_INPUT` | `1` |
| `SCARLET_QEMU_AUDIO` | `0` |
| `SCARLET_QEMU_AUDIO_DRIVER` | `coreaudio` (set an appropriate backend on Linux) |
| `SCARLET_QEMU_QMP` | Unset; optionally names a Unix socket |
| `SCARLET_QEMU_SNAPSHOT` | `0` |
| `SCARLET_GDB_PORT` | `12345` |

Boolean switches use `1` and `0`. `SCARLET_CMDLINE` overrides the boot command
line. `SCARLET_QEMU_HOSTFWD` accepts comma-separated QEMU forwarding rules;
an empty value disables forwarding. Additional arguments after the SDK's `--`
are passed to QEMU; put `--debug` first when using it.

The native SKK service supplies Japanese input. The project disables the
Linux-backed Mozc service because RV32 currently has no Linux ABI or Linux
userspace bundle.
