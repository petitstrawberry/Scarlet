# QEMU runner configuration

This page describes the environment variables read by this repository's
project runners. They are not `cargo-scarlet` CLI options. They apply whether
the runner is reached through `cargo make run-*` or `cargo scarlet run
--project ...`. Start in the Nix shell described in the
[development guide](README.md#development-environment).

The scope labels used below refer to these scripts:

| Scope | Project runner |
| --- | --- |
| AArch64 full | [aarch64-limine-full](../../projects/aarch64-limine-full/tools/run_aarch64.sh) |
| RISC-V full | [riscv64-limine-full](../../projects/riscv64-limine-full/tools/run.sh) |
| Microvm | [aarch64-limine-microvm](../../projects/aarch64-limine-microvm/tools/run_aarch64.sh) |
| Full | Both full projects, not microvm |
| All | All three project runners |

Defaults apply when a variable is unset or empty, except where noted.
For boolean switches, use `1` to enable and `0` to disable; most also accept
`true`. `SCARLET_DEBUG_MODE` requires `true`, while `SCARLET_QEMU_SNAPSHOT`
requires `1`. A variable documented for one runner is not necessarily read
by another. These are not the legacy scripts under `kernel/tools/` or the
kernel test runners.

## Explicit CPU acceleration

Every runner defaults to `SCARLET_QEMU_ACCEL=tcg`. No host accelerator is
automatically selected, and a failed explicit selection is not retried with
TCG. Display GL is independent of this choice.

### HVF on Apple Silicon macOS

Use the AArch64 full project and the QEMU supplied by the Nix shell:

```sh
SCARLET_QEMU_ACCEL=hvf cargo make run-aarch64
```

This requires host Hypervisor.framework support and a QEMU build with HVF.
The full runner selects `-cpu host` unless `SCARLET_QEMU_CPU_AARCH64` is
specified, and prefers the HVF firmware paths supplied by the environment.
HVF does not run the RISC-V guest or an AArch64 guest on an Intel Mac.

### KVM on Linux

Use a host compatible with the guest CPU architecture, a kernel exposing
usable KVM, permission to open `/dev/kvm`, and a QEMU build with KVM support:

```sh
# AArch64 Linux host, AArch64 guest.
SCARLET_QEMU_ACCEL=kvm cargo make run-aarch64

# RISC-V Linux host, RISC-V guest.
SCARLET_QEMU_ACCEL=kvm cargo make run-riscv64
```

The full runners select `-cpu host` for KVM. On x86 Linux, use TCG for both
Scarlet architectures; the presence of `/dev/kvm` does not make cross-ISA
hardware virtualization possible.

To inspect the accelerators compiled into the target QEMU, run
`qemu-system-aarch64 -accel help` or `qemu-system-riscv64 -accel help`.
That list, and `/dev/kvm` access permissions, do not prove that KVM can
initialize and create a VM in the current environment. If an explicit KVM
or HVF launch fails, QEMU reports the failure. Retry with
`SCARLET_QEMU_ACCEL=tcg` or remove the override.

See QEMU's [accelerator overview](https://www.qemu.org/docs/master/system/introduction.html#virtualisation-accelerators)
for the distinction between hardware virtualization and TCG emulation.

### CPU and machine variables

| Variable | Scope | Default | Effect |
| --- | --- | --- | --- |
| `SCARLET_QEMU_ACCEL` | All | `tcg` | Passed to `-accel`; explicitly select `tcg`, `kvm`, or `hvf` as appropriate. |
| `SCARLET_QEMU_SMP` | All | Full: `4`; microvm: `1` | Passed to `-smp` to configure virtual CPUs. Defaults are fixed, not derived from the host CPU count. |
| `SCARLET_QEMU_MEMORY` | All | `8G` | Guest RAM size, passed to `-m`. Also sizes shared memory when vhost-user video is enabled. |
| `SCARLET_QEMU_MACHINE_AARCH64` | AArch64 full, microvm | `virt,gic-version=3,acpi=off` | Replaces the `-machine` string, including machine options. |
| `SCARLET_QEMU_MACHINE_RV64` | RISC-V full | `virt,acpi=off` | Replaces the `-machine` string, including machine options. |
| `SCARLET_QEMU_CPU_AARCH64` | AArch64 full, microvm | Full: `host` with KVM/HVF, otherwise `max`; microvm: `max` | Replaces the `-cpu` value. Microvm does not automatically switch its CPU model for hardware acceleration. |
| `SCARLET_QEMU_VIRTUALIZATION` | All | AArch64 full: `1` with TCG, `0` with KVM/HVF; microvm: `1`; RISC-V full: `0` | Requests appending `virtualization=on` to the machine string. An existing `virtualization=` machine option takes precedence. The runners skip appending it for HVF. This does not select the host accelerator. |

RISC-V full leaves the CPU model at QEMU's default under TCG and explicitly
uses `host` under KVM; it has no CPU-model environment override.

Guest virtualization extensions and host acceleration are different
settings. In particular, running Scarlet's own hypervisor requires nested
virtualization support when the outer QEMU uses KVM/HVF. The full AArch64
runner does not request those extensions by default under hardware
acceleration. Follow the [microvm project guide](../../projects/aarch64-limine-microvm/README.md)
for its TCG-based nested-guest workflow; the full-project KVM/HVF examples
above are not a replacement for that setup.

## Display, GPU, input, and audio

Full projects use these automatic display defaults:

- macOS: `cocoa,gl=on,retina=on,full-grab=on`.
- Linux with `DISPLAY` or `WAYLAND_DISPLAY` set:
  `gtk,gl=on,grab-on-hover=on`, or
  `sdl,gl=on` if GTK is unavailable in QEMU.
- Without a local GUI session or supported Linux GUI backend: `vnc=:0`.

The Linux selection queries QEMU's compiled display backends, not whether
the host's GL stack will successfully initialize. GL displays select
`virtio-gpu-gl-pci`; non-GL displays select `virtio-gpu-pci`. Microvm defaults
to `none` for both display and GPU. Serial output remains in the terminal.

| Variable | Scope | Default | Effect |
| --- | --- | --- | --- |
| `SCARLET_QEMU_DISPLAY` | All | Full: host-based selection above; microvm: `none` | Replaces the entire `-display` value, including options. Explicit values do not inherit the default display options listed above. |
| `SCARLET_QEMU_GPU` | All | Full: display-based selection above; microvm: `none` | GPU `-device` value, or `none` to omit the GPU. Explicit values override display-based selection. |
| `SCARLET_QEMU_INPUT` | Full | `1` | Adds virtio keyboard and mouse devices. |
| `SCARLET_QEMU_AUDIO` | All | `0` | Adds a virtio sound device and a host audio backend. |
| `SCARLET_QEMU_AUDIO_DRIVER` | All | `coreaudio` | QEMU audio backend used when audio is enabled. This default is not selected by host OS; Linux users must choose an available backend. |
| `SCARLET_QEMU_SERIAL` | Full | `mon:stdio` | Replaces the QEMU serial backend. Microvm keeps `mon:stdio`. |

Input capture differs by backend:

- Cocoa's `full-grab=on` captures system key combinations and requires
  macOS Accessibility permission for QEMU.
- GTK's `grab-on-hover=on` grabs the keyboard when the pointer enters the
  guest display and releases it when the pointer leaves. It controls when
  keyboard grabbing is requested, rather than enabling Cocoa's global
  key-event capture. See [QEMU display options](https://www.qemu.org/docs/master/system/invocation.html#display-options)
  for these backend-specific settings.
- SDL already enables keyboard grabbing and disables Alt-Tab passthrough
  while grabbed in [QEMU's SDL backend](https://github.com/qemu/qemu/blob/master/ui/sdl2.c).
  It has no separate `full-grab` display option. Keep the normal SDL grab
  behavior; the default toggle is Ctrl-Alt-G.

Examples:

```sh
# No graphical display or VNC server; keep serial output.
SCARLET_QEMU_DISPLAY=none cargo make run-aarch64

# Explicit VNC instead of a local GUI.
SCARLET_QEMU_DISPLAY='vnc=:0' cargo make run-aarch64

# Cocoa with GL but without Retina.
SCARLET_QEMU_DISPLAY='cocoa,gl=on,retina=off,full-grab=on' cargo make run-aarch64

# Disable GL while keeping Retina.
SCARLET_QEMU_DISPLAY='cocoa,gl=off,retina=on,full-grab=on' \
SCARLET_QEMU_GPU=virtio-gpu-pci \
cargo make run-aarch64
```

`SCARLET_QEMU_DISPLAY=none` alone does not remove the guest GPU; set
`SCARLET_QEMU_GPU=none` too if that is intended. When selecting a display
explicitly, include compatible GL options if using `virtio-gpu-gl-pci`.
See [QEMU display options](https://www.qemu.org/docs/master/system/invocation.html#display-options)
for backend settings. Scarlet's macOS defaults use the QEMU fork supplied
by this repository's Nix environment.

## Networking

These variables apply only to full projects. Networking uses QEMU's user
backend, with either a virtio NIC or USB CDC-NCM.

| Variable | Default | Effect |
| --- | --- | --- |
| `SCARLET_QEMU_NET` | `1` | Enables the runner's network backend/device. |
| `SCARLET_QEMU_HOSTFWD` | Per-project rules below | Comma-separated QEMU forwarding rules, without the `hostfwd=` prefix. Replaces the entire default list. An empty value restores the defaults, not an empty list. |
| `SCARLET_QEMU_REMOTE_DESKTOP_HOST_PORT` | `5900`, or `5901` when QEMU display is `vnc=:0` | AArch64 full only. Host port forwarded to Scarlet's port 5900. Used only with the default forwarding list. |
| `SCARLET_QEMU_SSH_HOST_PORT` | `2222` | AArch64 full only. Loopback host port forwarded to guest port 22. Used only with the default forwarding list. |
| `SCARLET_QEMU_USB_NCM` | `0` | Uses USB CDC-NCM instead of the virtio NIC and adds xHCI as needed. Requires `SCARLET_QEMU_NET=1`. |
| `SCARLET_QEMU_USB_NCM_MAC` | `52:54:00:12:34:57` | MAC address for the USB NIC. |
| `SCARLET_QEMU_USB_NCM_PCAP` | `<project>/.scarlet/usb-ncm-<arch>.pcap` | USB-NCM capture path, where `<arch>` is `aarch64` or `riscv64`. Unlike most overrides, an explicitly empty value disables capture. |

Both full projects forward TCP 8080, UDP 8080, and UDP 1234 to the same guest
ports. AArch64 full also forwards the remote-desktop host port to guest 5900
and `127.0.0.1:<SSH host port>` to guest 22. QEMU's own VNC server is
separate from Scarlet's remote-desktop service; the AArch64 default avoids
their host-port collision by moving the latter to 5901 when necessary.

The default rules other than SSH omit a host bind address. Use explicit
loopback rules when services should not be exposed on other host interfaces:

```sh
SCARLET_QEMU_HOSTFWD='tcp:127.0.0.1:2222-:22,tcp:127.0.0.1:8080-:8080' \
cargo make run-aarch64
```

## Storage and project paths

These variables apply only to full projects unless a narrower scope is
listed. `<project>` means the runner's project directory.

Both full projects attach one GPT disk containing the EFI boot partition and
the ext2 root partition at `/dev/vblk0p2`. Their default rootfs transport is
`none` because no additional rootfs disk is needed.

| Variable | Scope | Default | Effect |
| --- | --- | --- | --- |
| `SCARLET_QEMU_PROJECT_DIR` | Full | Runner's project directory | Changes where the runner looks for its manifest and project artifacts. Does not select a different project for the preceding build. |
| `SCARLET_QEMU_ROOTFS_TRANSPORT` | Full | `usb` when `scarlet.toml` contains a `cmdline` with `root=/dev/usbblk0`; otherwise `none` | Selects `usb`, `virtio`, or `none` for the rootfs disk attachment. Does not change the kernel command line. |
| `SCARLET_QEMU_USB_STORAGE` | Full | `1` when rootfs transport is `usb`; otherwise `0` | Adds a USB mass-storage device and xHCI controller. |
| `SCARLET_QEMU_USB_STORAGE_IMAGE` | Full | Rootfs image for USB rootfs; otherwise `<project>/.scarlet/images/qemu-usb-storage.img` | Raw image attached to the USB storage device. |
| `SCARLET_QEMU_USB_STORAGE_SIZE` | Full | `64M` | Size used to create a missing USB storage image; existing images are not resized or reformatted. |
| `SCARLET_QEMU_EMMC` | AArch64 full | `0` | Adds SDHCI PCI and eMMC devices; requires these device models in QEMU. |
| `SCARLET_QEMU_EMMC_IMAGE` | AArch64 full | `<project>/.scarlet/images/qemu-emmc.img` | Raw eMMC image path. |
| `SCARLET_QEMU_EMMC_SIZE` | AArch64 full | `4G` | Size used to create a missing eMMC image; existing images are retained. |

New auxiliary storage images are blank sparse files, not formatted
filesystems. Disabling USB storage while the kernel expects USB rootfs, or
changing the transport without adjusting the project command line, can
prevent the root filesystem from mounting. Microvm attaches its fixed
project rootfs through virtio and does not read these storage overrides.

## Firmware and EFI variables

The Nix environment supplies firmware paths. The runners use the first
existing file in their selection order:

- AArch64: HVF-specific path when using HVF, then the EL2 path, then the
  generic ARM64 path, then the runner's `/usr/share` candidates.
- RISC-V: the RV64 environment path, then the runner's `/usr/share`
  candidates.

Firmware code and variable templates are selected separately. Use matching
images from the same firmware build.

| Variable | Scope | Effect |
| --- | --- | --- |
| `SCARLET_EFI_CODE_ARM64_HVF`, `SCARLET_EFI_VARS_ARM64_HVF` | AArch64 full, microvm | Highest-priority code/template pair for HVF. |
| `SCARLET_EFI_CODE_ARM64_EL2`, `SCARLET_EFI_VARS_ARM64_EL2` | AArch64 full, microvm | Next-priority AArch64 code/template pair. |
| `SCARLET_EFI_CODE_ARM64`, `SCARLET_EFI_VARS_ARM64` | AArch64 full, microvm | Generic AArch64 code/template pair. |
| `SCARLET_EFI_CODE_RV64`, `SCARLET_EFI_VARS_RV64` | RISC-V full | RISC-V code/template pair. |
| `SCARLET_EFI_VARS_PERSIST` | All | Default `0`: use a temporary writable template copy and remove it on exit. `1` or `true` retains the per-project runtime file between runs. |
| `SCARLET_EFI_VARS_RUNTIME_ARM64` | AArch64 full | Use this existing writable runtime file directly, instead of copying a template. Takes precedence over `SCARLET_EFI_VARS_PERSIST`. |
| `SCARLET_EFI_VARS_RUNTIME_RV64` | RISC-V full | Same explicit-runtime behavior for RISC-V. |

Default persistent files are `<project>/.scarlet/AAVMF_VARS.fd` for
AArch64 full, `<project>/.scarlet/RISCV_VIRT_VARS.fd` for RISC-V full, and
`<project>/.scarlet/images/AAVMF_VARS.fd` for microvm. The explicit runtime
overrides are not read by microvm. Microvm also sets the EFI boot timeout
to zero when `virt-fw-vars` is available.

## Debugging and local automation

| Variable | Scope | Default | Effect |
| --- | --- | --- | --- |
| `SCARLET_DEBUG_MODE` | All | `false` | `true` enables `-gdb tcp::12345 -S`. The runner's `--debug` argument also enables this mode. This is separate from choosing a debug build. |
| `SCARLET_QEMU_DEBUG_FLAGS` | All | Unset | Comma-separated flags for QEMU `-d`. Takes precedence over `SCARLET_QEMU_GUEST_ERRORS`. |
| `SCARLET_QEMU_GUEST_ERRORS` | All | `0` | Enables `-d guest_errors` when explicit debug flags are absent. |
| `SCARLET_QEMU_GUEST_ERRORS_LOG` | All | `<repo>/qemu-guest-errors-<arch>.log` | Log path when the effective debug flags are exactly `guest_errors`. |
| `SCARLET_QEMU_DEBUG_LOG` | All | `<repo>/qemu-debug-<arch>.log` | Log path for other debug-flag combinations. |
| `SCARLET_QEMU_CHECK_TEST_OUTPUT` | All | `0` | Captures QEMU stdout and checks the Scarlet test-runner pass/fail markers after exit. Not needed for ordinary desktop runs. |
| `SCARLET_QEMU_QMP` | Full | Unset | Unix socket path for QMP, configured with `server=on,wait=off`. |
| `SCARLET_QEMU_SNAPSHOT` | Full | `0` | `1` passes QEMU `-snapshot` for temporary disk changes. Does not stop the preceding image build from writing artifacts. |

In the log defaults, `<arch>` is `aarch64` or `riscv64`. The current runners
assemble `-d`/`-D` arguments through shell word splitting, so use debug-log
paths without whitespace.

## Vhost-user video

These advanced variables apply to full projects, not microvm. They configure
the separate video decode device, not the display GPU or GL renderer.

| Variable | Default | Effect |
| --- | --- | --- |
| `SCARLET_VHOST_USER_VIDEO` | `0` | Enables the vhost-user video PCI device and shared guest memory. |
| `SCARLET_VHOST_USER_VIDEO_SOCKET` | `/private/tmp/scarlet-video.sock` | Unix socket connecting QEMU to the video daemon. |
| `SCARLET_VHOST_USER_VIDEO_DAEMON` | `<repo>/dist/host/vhost_video_videotoolbox` | Host daemon executable. |
| `SCARLET_VHOST_USER_VIDEO_LOG` | `/private/tmp/scarlet-video.log` | Daemon log path; `stderr` leaves daemon output attached to the terminal. |
| `SCARLET_VHOST_USER_VIDEO_ID` | `31` | Virtio device ID; keep it consistent with the guest driver. |
| `SCARLET_VHOST_USER_VIDEO_QUEUES` | `2` | Number of virtqueues. |
| `SCARLET_VHOST_USER_VIDEO_QUEUE_SIZE` | `256` | Virtqueue size. |
| `SCARLET_VHOST_USER_VIDEO_CONFIG_SIZE` | `64` | Device configuration-space size in bytes. |
| `SCARLET_VHOST_USER_VIDEO_SOCKET_WAIT_ATTEMPTS` | `100`; `1200` for Swift fallback | RISC-V full only. Number of socket-readiness polls, 50 ms apart, after launching the daemon. |

AArch64 full starts the configured executable on macOS and waits up to
100 polls for its socket; on other hosts, supply an already running daemon.
RISC-V full reuses an existing socket, otherwise starts the executable. On
macOS, it can fall back to `swift run` from the repository's VideoToolbox
package if the executable is missing. Automatically launched daemons are
stopped by the runner on exit. The default daemon and `/private/tmp` paths
are macOS-oriented; other hosts need a compatible daemon and suitable paths.

See the [video device documentation](../device/video-decode.md) for the
guest-side interface.
