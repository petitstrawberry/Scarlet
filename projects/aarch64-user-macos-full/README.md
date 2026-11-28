# AArch64 user-mode Scarlet on macOS

This project builds its own initramfs and root filesystem from the shared
Scarlet bundle definitions and the project's root filesystem overlay. It
includes the desktop, prebuilt Myrica, Debian environment, experimental
applications, Rust toolchain, rust-analyzer, and prototype static module. Its
BSP targets Apple Silicon macOS and supplies host primitives for the
[user-mode kernel](../../docs/architecture/user-mode-kernel.md).

From the repository root on Apple Silicon, enter the repository's Nix dev
shell and build the project with Cargo Scarlet:

```sh
cargo scarlet image --project projects/aarch64-user-macos-full --release --locked
cargo scarlet run --project projects/aarch64-user-macos-full --release --locked --no-image
```

`image` builds the native BSP, resolves the guest layers, and writes the CPIO
and ext2 files under this project's `.scarlet/images`. The `run` command
dispatches to the project's manifest runner, which passes those outputs to
Scarlet. Normal boot schedules `/init` in a native address-space worker.
It mounts the ext2 root and starts `/bin/stemd` under `scarlet-ld`.
Append `-- --inspect-root` to check both images through the guest ELF loader.
The default runner opens a macOS window and attaches keyboard and pointer
input. The serial console remains available in the launching terminal.
Set `SCARLET_HOSTED_GUI=0` to run with the serial console alone. The BSP
reserves an 8 GiB guest RAM arena and boots four vCPUs by default.

Run the native tests with:

```sh
cargo test --locked --manifest-path kernel/hosted/runtime/Cargo.toml
cargo test --locked --manifest-path projects/aarch64-user-macos-full/bsp/Cargo.toml
```

The tests cover migration between host threads, shared RAM aliases and
invalidation, vCPU timer and IPI state, host wall-clock and entropy setup, and
run permit retirement during mapping changes and worker termination. The
hosted CPU test runs Scarlet kernel tasks after all vCPUs are online with 1, 2, 4, and
16 vCPUs. Each task completes a durable root block batch through the hosted
IRQ path and checks timer wakeups and IPI acknowledgements. Runtime tests
hold a secondary CPU at the startup gate and exercise failures on both sides
of the preparation boundary.

After building the release image, run the full console regression from the
repository root. It boots a private copy twice, signs in through tty0, checks
a native file write after reopening the disk, executes a Debian shell
pipeline, and closes each instance through the host terminal escape.

```sh
python3 projects/aarch64-user-macos-full/tools/test_full_console.py
```

The instruction regression boots freestanding guests through the release BSP.
It checks a sectionless ELF, anonymous JIT code, and 32-bit and 64-bit register
pairs on native and mixed-permission pages.

```sh
python3 projects/aarch64-user-macos-full/tools/test_guest_instructions.py
```

The host serial driver feeds the existing Scarlet `tty0` through the
interrupt controller. Ctrl-C goes to the guest foreground process group.
Press Ctrl-], then lowercase `q` to close the instance and restore the host
terminal settings. Press Ctrl-] twice to send a literal Ctrl-] to the guest.
Other key combinations pass through to the guest.
Disconnect wakes guest readers so they can drain buffered input and reach EOF.
The console belongs to one instance. A new instance creates a new terminal.
The native PTY tests exercise these paths.

The BSP registers the default macOS audio output during the kernel's late
initcalls. Core Audio consumes 48 kHz stereo S16LE periods from a bounded
host queue. Its render callback reports completed periods through the hosted
IRQ route so the existing Scarlet audio worker can refill the queue.
On a Mac with an active default output, run the native playback test with:

```sh
cargo test --locked --manifest-path projects/aarch64-user-macos-full/bsp/Cargo.toml --test hosted_audio -- --ignored
```

The test drains and refills the PCM ring through `audio0` using the same
parameters as SAS. It sends silence and also checks stop and restart.

The macOS BSP registers `eth0` with the existing Scarlet Ethernet and IPv4
stack. The Nix shell pins the libslirp library used by a host I/O thread.
That thread exchanges Ethernet frames through bounded queues, with receive
and transmit notifications entering Scarlet through a virtual IRQ. With an
active IPv4 route, run the DHCP and TCP echo test with:

```sh
cargo test --locked --manifest-path projects/aarch64-user-macos-full/bsp/Cargo.toml --test hosted_network -- --ignored
```

The test obtains the NAT's lease over Scarlet UDP and uses it for a Scarlet
TCP connection to a temporary peer on the Mac.

Direct execution is still gated by the host's VM and register behavior.
The ordinary fixed-address build of `/init` had its entry at `0x222104`,
below Darwin's 4 GiB page-zero floor. The macOS initramfs links the same
bootstrap source at `0x7800000000` with 16 KiB segment boundaries.
On Darwin 25.6.0, protecting one 4 KiB
subrange with `mach_vm_protect` changed permissions for all 16 KiB. Native
instructions also lost a live x18 value across host preemption, without a
guest syscall or explicit breakpoint. The
worker keeps guest x18 in the saved thread context for instructions it
rewrites. The project's userspace Cargo config also reserves x18 for newly
compiled Scarlet Rust code. Existing ELF files do not require this build
flag. Compatibility with mixed-permission host pages and the full range of
guest instructions still needs verification.

The hosted ELF loader selects 16 KiB initial load pages for Scarlet
executables using `/bin/scarlet-ld` when their segment alignment, layout
and RELRO allow it. Segment-end padding inherits that segment's initial
permissions. Images with conflicting protection ranges use 4 KiB load
pages. Subsequent explicit protection changes and unmaps use 4 KiB guest
ranges in both modes.

Apple's [XNU spawn header](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/spawn.h#L65-L72)
defines its 4 KiB process mode for private debug and development builds.
On Darwin 25.6.0 release, a native arm64 probe started normally and reported
16 KiB pages. Starting the same probe with the 4 KiB spawn flag returned
`EBADMACHO`.

The earlier macOS prototype and its execution notes are in the local
`backup/632-before-rewrite-3e2203df` branch:

```sh
git show backup/632-before-rewrite-3e2203df:projects/aarch64-hosted-macos/README.md
git show --stat 4880da85b
```

That prototype built a hosted variant of `/init`, linked the interpreter at
`0x7801800000` with 16 KiB alignment, and rejected overlapping mappings after
rounding ELF segments to host pages. Its execution copy also replaced guest
TLS register instructions with breakpoints. Its Cargo configuration reserved
x18 when compiling Scarlet AArch64 Rust applications.
