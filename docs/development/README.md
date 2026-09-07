# Scarlet development

This guide covers the development environment and build/run tasks for this
repository. The tasks are defined in the root [Makefile.toml](../../Makefile.toml).
For SDK commands, project manifests, and image composition, see the separate
[project tooling guide](../build-system/README.md).

## Development environment

Use the Nix development shell from the Scarlet repository root:

```sh
nix develop
```

With `direnv` and `nix-direnv`, entering the checkout can be reduced to:

```sh
direnv allow
```

Manual setup is not the supported path. It must provide an equivalent Scarlet
Rust toolchain, `cargo-make`, `cargo-scarlet`, QEMU, cross tools, filesystem
image tools, firmware paths, fontconfig, and the other tools described in
[flake.nix](../../flake.nix).

## Build and run

Run these tasks from the Scarlet repository root in the Nix development shell:

```sh
# Build kernel and core user components (image composition is separate).
cargo make build-riscv64
cargo make build-aarch64

# Build images and run through the project runner.
cargo make run-riscv64
cargo make run-aarch64
cargo make run-aarch64-microvm
```

The `build-*` tasks do not compose the complete project image. The `run-*`
tasks compose release images and launch the runner; `run-debug-*` selects the
debug build. The `debug-riscv64` and `debug-aarch64` tasks start QEMU paused
with a GDB server. See [userspace development](../userspace/README.md) for
application-only builds.

## QEMU acceleration

Full projects automatically select hardware acceleration when the host and
guest CPU architectures match: HVF on Apple Silicon macOS, or KVM on Linux.
The runner checks QEMU's supported accelerators and the host's HVF support
or read/write access to `/dev/kvm`. Otherwise it uses TCG, including when
running AArch64 or RISC-V Scarlet on an x86 host. KVM and HVF use `-cpu host`.

`SCARLET_QEMU_ACCEL` overrides this selection. For example, to force CPU
emulation:

```sh
SCARLET_QEMU_ACCEL=tcg cargo make run-aarch64
```

The AArch64 full project exposes guest virtualization extensions by default
only under TCG. KVM/HVF acceleration does not by itself provide nested
virtualization for Scarlet's hypervisor. The microvm project retains its
TCG default because it runs guests inside Scarlet.

## QEMU display

Full projects open a QEMU GUI by default: Cocoa on macOS, or GTK (with SDL as
the fallback) on Linux when `DISPLAY` or `WAYLAND_DISPLAY` is set. The Linux
runner checks which backends the selected QEMU supports. Without a local GUI
session or supported GUI backend, it keeps the VNC display (`vnc=:0`). Serial
output stays in the terminal. The same selection applies to debug runs.
Local GUI defaults include `gl=on` and `virtio-gpu-gl-pci`, independently of
whether the CPU uses TCG, KVM, or HVF. VNC and non-GL displays keep
`virtio-gpu-pci`. The GPU-less microvm project remains headless by default.

`SCARLET_QEMU_DISPLAY` overrides automatic selection, including display options.
For example:

```sh
# Serial console only, without a graphical display or VNC server.
SCARLET_QEMU_DISPLAY=none cargo make run-aarch64

# Use VNC explicitly instead of a local window.
SCARLET_QEMU_DISPLAY='vnc=:0' cargo make run-riscv64
```

`SCARLET_QEMU_GPU` overrides the display-based GPU selection. For example,
to explicitly disable GL on macOS:

```sh
SCARLET_QEMU_DISPLAY='cocoa,gl=off' \
SCARLET_QEMU_GPU=virtio-gpu-pci \
cargo make run-aarch64
```

See [QEMU display options](https://www.qemu.org/docs/master/system/invocation.html#display-options)
for backend-specific settings.

## Cross C compiler selection

The Nix development shell and Docker image provide unwrapped Clang as
`TARGET_CC`, the cc-rs fallback for cross compilation. Native C builds keep
using `HOST_CC`/`CC` and the Nix compiler wrapper. Avoid exporting global
`CC_<target>` defaults: they override application settings in Cargo's `[env]`
table, including `yt-for-scarlet`'s cross GCC selection.

Applications can select a different cross compiler with `CC_<target>` in
their own `.cargo/config.toml`; cc-rs checks that before `TARGET_CC`. This
controls C headers and compilation only, not Rust `std` support. The existing
`cargo make test-cross-cc` task checks Scarlet targets, application overrides,
and native compiler selection in the Nix shell. After updating the flake,
reload direnv or re-enter the shell to discard old environment variables.

## Formatting, linting, and tests

Run these from the repository root in the Nix development shell:

```sh
# Formatting
cargo make fmt
cargo make fmt-check

# Clippy
cargo make clippy-riscv64
cargo make clippy-aarch64

# Kernel tests
cargo make test-riscv64
cargo make test-aarch64
```

The root test tasks run the kernel tests under QEMU and require the same Nix
shell environment as normal runs. CI currently expects both `test-riscv64` and
`test-aarch64` to pass. To run an individual kernel test:

```sh
cd kernel
cargo test --target targets/riscv64gc-unknown-none-elf.json test_name
```

## Rust documentation

From the repository root:

```sh
cargo make doc-riscv64
cargo make doc-kernel
cargo make doc-userlib
```

## See also

- [Project tooling](../build-system/README.md)
- [Kernel development](../kernel/README.md)
- [Userspace development](../userspace/README.md)
- [Distribution model](../architecture/distro-model.md)
