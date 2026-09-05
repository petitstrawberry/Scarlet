# SGFX desktop integration image

This focused AArch64 image boots SWS, Scarlet's desktop shell, and the
std-enabled `ui-smoke` application. The application catalog also contains
`UI Smoke` and `SGFX Showcase` (`ui-sgfx-showcase`) for interactive testing.
It uses the current Scarlet Cargo graph, without sibling-checkout overrides
or the external application repositories in the full desktop bundle.

This is a runtime test fixture, not the release desktop image. It reuses the
full AArch64 project's BSP and UEFI runner. It does not add a Cargo package.

## Build and run

From the Scarlet repository root, use the pinned Scarlet Rust dev shell:

```sh
nix develop .#default
cargo make image-sgfx-smoke-aarch64
cargo make run-sgfx-smoke-aarch64
```

The runner defaults to `virtio-gpu-gl-pci`, snapshot disk writes, and no
network device or port forwarding. On macOS it selects `cocoa,gl=on`.
Elsewhere, set `SCARLET_QEMU_DISPLAY` to a GL-capable display supported by
your QEMU build; check `qemu-system-aarch64 -display help`. An ordinary
non-GL VNC display is not sufficient for this VirGL test.

The image and runtime EFI variables live under this project's `.scarlet/`,
separate from the full project's images. The inherited runner options remain
available, including `SCARLET_QEMU_MEMORY`, `SCARLET_QEMU_SMP`, and
`SCARLET_QEMU_ACCEL`. `SCARLET_QEMU_SNAPSHOT=0` opts into persistent guest
disk writes; otherwise discard guest changes when QEMU exits.

## Capture and control

For isolated automation, create a private artifact directory before starting
QEMU, and keep the same environment in a second terminal:

```sh
sgfx_artifacts=$(mktemp -d /tmp/scarlet-sgfx.XXXXXX)
export SCARLET_QEMU_QMP="$sgfx_artifacts/qmp.sock"
export SCARLET_QEMU_SERIAL="file:$sgfx_artifacts/serial.log"
cargo make run-sgfx-smoke-aarch64
```

`SCARLET_QEMU_SERIAL` defaults to `mon:stdio` for an interactive UART shell.
A file backend records output only; use the default shell, or a local Unix
serial backend, when guest commands such as `logctl` are needed.

From the second terminal:

```sh
cargo make qmp
QMP_COMMAND=query-mice cargo make qmp
QMP_COMMAND=send-key \
  QMP_ARGUMENTS='{"keys":[{"type":"qcode","data":"spc"}]}' cargo make qmp
QMP_COMMAND=quit cargo make qmp
```

The helper negotiates QMP capabilities, matches response IDs, ignores
asynchronous events, and fails on command errors, disconnects, and timeouts.
`cargo make test-qmp` tests the transport without a guest and also runs in CI.
A successful QMP response proves only that QEMU accepted the command: inspect
the guest display and logs before marking an interaction successful.

QMP `screendump` can fail with `no surface` on the tested VirGL/Cocoa build.
Capture the QEMU window through the host OS in that case; do not substitute
a software GPU and report that as VirGL evidence. Record the guest resolution
and output scale when using pointer coordinates.

## Manual release scenarios

### Native asynchronous GPU diagnostic

Inside the guest, run `/bin/gpu-async-smoke`. It requires VirtIO/VirGL and uses
the normal Scarlet Rust `std` userspace target. Success requires the literal
`[gpu-async-smoke] ALL PASS` marker and exit status zero, not merely a booting
desktop. It checks an async clear with exact pixel readback, queue checkpoints,
detach ordering, dropped completion handles, and completion after closing the
queue, context and image owners. It does not exercise the still-synchronous
SGFX native facade or certify A618, imported-image reuse, or fault recovery.

The diagnostic is installed but not automatically started. For automation in
a disposable verification image, add a one-shot `stemd` service with
`exec = "/bin/gpu-async-smoke"` and `tty = "/dev/tty0"`. Keep that autostart
configuration out of the normal desktop. The actual 2026-09-06 checks used a
release image, Cocoa GL, TCG and both PCI/two CPUs and MMIO/one CPU. To exercise
MMIO, set `SCARLET_QEMU_GPU=virtio-gpu-gl-device` before running the image.

### Desktop scenarios

1. Confirm SWS readiness and
   `[ScarletUI] platform-sws renderer=sgfx backend=scarlet-virgl` in the
   `ui-smoke` UART output. Treat an application error as a failure even if
   the demonstration process exits with status zero.
2. Click Increment; check both `Counter: 1` and `[ui-smoke] counter=1`.
   Type text into the field and confirm the `Input:` label changes with it.
   Exercise the toggle and window decorations.
3. Launch SGFX Showcase from the catalog, or run `/bin/ui-sgfx-showcase`
   from the guest UART shell. Open the textured cube, gears, and mesh swarm.
   Compare successive frames and the FPS HUD; a single static image is not
   evidence of continued submission.
4. Resize, maximize, and restore a demo window. Close and reopen its scene,
   then repeat while another scene is rendering. Check for blank frames,
   stale content, application errors, and guest panics.
5. Quit QEMU and repeat from a fresh snapshot. Keep failed attempts alongside
   successful runs; one successful boot does not establish startup stability.

Record the exact Scarlet revision, `.cargo/Cargo.lock`, `flake.lock`, project
configuration, commands, serial logs, and screenshots with the result. The
[release baseline](../../docs/release/1.0-baseline.md) distinguishes verified
scenarios from open runtime and full-image release gates.
