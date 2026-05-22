# pdfview Debug Notes

Date: 2026-05-22

## Ground Rules

- Use `cargo make` for Scarlet build/run verification.
- Keep Linux userland/buildroot work separate from kernel debugging.
- Do not add new kernel or VM changes for PDF display without first recording:
  - observed fact
  - hypothesis
  - exact proposed change
  - expected result
  - rollback plan

## Current Strategy

- Do not maintain a custom PDF renderer.
- Use the existing Linux ABI path to launch zathura on AArch64.
- Use GTK/Wayland because `gtk3-demo` has already been confirmed to work.
- Keep `pdfview` as a Scarlet-native launcher only:
  - translate Scarlet-visible file paths;
  - select the `linux-aarch64` ABI;
  - set the Linux viewer environment;
  - start zathura in a minimal GUI mode;
  - `execve` `/scarlet/system/linux-aarch64/usr/bin/zathura`.

## Current Facts

- `/system` is not the Scarlet shell path for deployed Linux userland.
- The deployed Linux rootfs is visible from Scarlet under
  `/scarlet/system/linux-aarch64`.
- After a Linux ABI process starts, paths are interpreted relative to that
  Linux root.
- `pdfview /root/test.pdf` must pass the PDF to zathura as
  `/scarlet/system/scarlet/root/test.pdf`, because `/root/test.pdf` is on the
  Scarlet side.
- The active shell environment is AArch64:
  - `PATH=/scarlet/system/scarlet/bin:/scarlet/system/linux-aarch64/bin:/scarlet/system/linux-aarch64/usr/bin`
  - `LD_LIBRARY_PATH=/scarlet/system/linux-aarch64/usr/lib:/scarlet/system/linux-aarch64/lib`
- The Wayland bridge is started by stemd through
  `mkfs/initramfs/system/scarlet/etc/stemd.d/services/02-wayland-bridge.toml`.
- `mkfs/initramfs/system/scarlet/root/test.pdf` is included in
  `mkfs/dist/initramfs-aarch64.cpio` as `system/scarlet/root/test.pdf`.
- `mkfs/dist/rootfs.img` contains:
  - `/system/linux-aarch64/usr/bin/zathura`
  - `/system/linux-aarch64/usr/lib/zathura/libpdf-poppler.so`

## Buildroot/Userland Notes

- Buildroot and Linux ABI user program generation are Linux-host tasks.
- macOS execution of the Buildroot helper scripts is stopped with guidance.
- The scripts keep Docker-compatible defaults:
  - RISC-V: `/opt/buildroot`
  - AArch64: `/opt/buildroot-aarch64`
  - prebuilt staging: `/opt/prebuilt`
- The same scripts also allow path injection through:
  - `BUILDROOT_DIR`
  - `PREBUILT_DIR`
  - `WORKDIR`

## Resolved Issues In This Pass

- `green` was not the right target for PDF display. It was only evidence that
  Linux-side graphics binaries could be launched.
- The custom `scarlet-pdfview` direction was abandoned.
- zathura-pdf-poppler initially installed its plugin under the Buildroot staging
  path instead of runtime `/usr/lib/zathura`; this is fixed with
  `-Dplugindir=/usr/lib/zathura`.
- `mke2fs -d "$ROOTFS_DIR"` copied host extended attributes from the source
  tree and could produce a bad rootfs image. The rootfs builder now feeds
  `mke2fs` a tarball created with `--no-xattrs`, preserving symlinks and modes
  while avoiding host xattrs.
- The first zathura runtime attempt reached GTK/Wayland but failed after
  `xkbcommon` could not load `/usr/share/X11/xkb`. Enabling Buildroot's
  `BR2_PACKAGE_XKEYBOARD_CONFIG` put staging-prefixed paths into the install
  check, so the helper script now installs xkeyboard-config data into the
  zathura runtime prefix and deploys `/usr/share/X11/xkb`.
- `pdfview` sets `GTK_USE_PORTAL=0` to avoid treating the settings portal as a
  hard dependency on this minimal userland.
- zathura then failed in libwayland:
  `wl_display_dispatch_queue()` asserted that `ppoll(..., timeout=NULL)` must
  return `-1` or a positive ready count. Scarlet's single-fd `sys_ppoll` path
  could return `0` after a stale/pending wake even with an infinite timeout.
  The fix is to re-wait until readiness, signal, or timeout. This is Linux ABI
  poll semantics, not a page cache or trap-vector issue.
- zathura/GTK also exposed missing Linux syscalls:
  - `getresuid` and `getresgid` now return root IDs through user pointers,
    matching the existing fixed-root `getuid`/`getgid` behavior.
  - `mremap` is present in the generic syscall table and returns `ENOSYS`, so
    libc can take its fallback path instead of hitting the invalid-syscall path.
- GTK/Wayland then exposed a local socket `sendmsg` data-copy bug. `sys_sendmsg`
  copied each iovec by translating only the first user page and slicing across
  `iov_len`; if a Wayland request header crossed a user page boundary, the
  bridge saw a valid first word followed by zeroed/invalid header fields
  (`object_id=18 opcode=0 size=0`). `sys_sendmsg` now copies iovec data with
  `copy_from_user` page by page in both the generic and RISC-V Linux ABI socket
  paths.
- After that fix, GTK no longer crashed the bridge, but pointer traffic exposed
  an event-packet mismatch. SWS sends input as evdev-style packets ending in
  `EV_SYN`; the Wayland bridge used to ignore `EV_SYN` and emitted
  `wl_pointer.motion + wl_pointer.frame` for each `ABS_X` and `ABS_Y`
  individually. The bridge now batches pending pointer motion/button messages
  and flushes them on `EV_SYN`, mapping the SWS packet boundary to
  `wl_pointer.frame`.
- The bridge now logs forwarded pointer enter/leave/motion/button/frame events,
  keyboard enter/leave events, and any object-id-zero input event while
  diagnosing input delivery.
- Pointer logs showed valid `wl_pointer.motion` and `wl_pointer.frame` bytes,
  but libwayland-client printed
  `discarded [unknown]#0.[event 0](0 fd, 0 byte)` during mouse movement. The
  server-side event bytes were correct, so the next fault boundaries were the
  Scarlet native `StreamWrite` path used by `wayland_bridge` and the Linux
  `recvmsg` path used by libwayland-client. Both had the same single-page
  user-buffer assumption as the earlier Linux `sendmsg` bug. `sys_stream_write`
  now copies the full user buffer with `copy_from_user`; Linux `sys_recvmsg`
  now copies stream data and atomic handle+data payloads back to user iovecs
  with `copy_to_user`.
- After input delivery worked, pointer movement was still very expensive:
  SWS emitted `ABS_X`, `ABS_Y`, and `EV_SYN` for every mouse position, queued
  every packet, and the bridge converted every queued packet into Wayland
  motion/frame traffic. SWS now coalesces only adjacent tail pointer-motion
  packets in its pending input queues, preserving all non-motion boundaries
  such as button, key, enter, and leave events. The bridge also batches the
  already-drained Wayland input messages into one stream write per loop instead
  of one write per Wayland event.
- Runtime logs then showed a separate repaint loop: GTK repeatedly requested a
  new `wl_surface.frame`, attached a new SHM buffer, committed, and immediately
  received `wl_callback.done`. The bridge now delays frame callback completion
  until the queued SWS window-buffer update is flushed, so the callback acts as
  a frame pacing point instead of an immediate repaint trigger.
- GTK compatibility is still partial. `weston-simple-shm`, `gtk3-icon-browser`,
  and parts of `gtk3-widget-factory` can run, but complex GTK UI can still lose
  click handling while hover continues. The strongest current hypothesis is
  missing Wayland popup/grab coverage (`xdg_surface.get_popup`,
  `xdg_wm_base.create_positioner`, and `xdg_popup.grab`). For the immediate PDF
  viewer goal, do not switch to image conversion; keep the GUI viewer path and
  make zathura avoid optional UI/database/cache surfaces as much as possible.
- `pdfview` now creates `/tmp/pdfview-zathura-{config,data,cache}` and launches
  zathura with:
  - `--config-dir=/tmp/pdfview-zathura-config`
  - `--data-dir=/tmp/pdfview-zathura-data`
  - `--cache-dir=/tmp/pdfview-zathura-cache`
  - `--mode=presentation`
  This keeps the first target as a real GUI zathura window while avoiding the
  default `/root/.local/share/zathura` database path and reducing the chance of
  exercising popup-heavy UI.
- `--plugins-dir=/usr/lib/zathura` was removed again because zathura appends it
  to the built-in default plugin directory rather than replacing the default,
  causing the PDF plugin to load twice and report
  `filetype already registered: application/pdf`.
- `pdfview` also writes `/tmp/pdfview-zathura-config/zathurarc` with
  `set database null` so zathura does not enter the bookmark/history sqlite
  path while the Linux ABI still lacks complete filesystem-stat coverage.
- The next AArch64 run failed before GTK startup because the dynamic linker
  could not find `libjson-glib-1.0.so.0` and `libsqlite3.so.0`. That was a
  Buildroot rootfs/package mismatch: girara/zathura were built against
  json-glib and sqlite, while `build_buildroot.sh` explicitly disabled both
  packages. AArch64 Buildroot now enables `BR2_PACKAGE_JSON_GLIB` and
  `BR2_PACKAGE_SQLITE`; the rootfs image was regenerated with both libraries.
- GTK also emits multiple thin damage rectangles for some redraws, for example
  one right-edge strip and one bottom-edge strip. The bridge and SWS used to
  collapse those into a single bounding rectangle, turning roughly 6.5 KiBpx of
  real damage into a roughly 310 KiBpx update. Both bridge-side pending damage
  and SWS compositor dirty damage now keep a bounded list of rectangles and
  merge only when the union area is close to the separate areas.

## Latest Runtime Result

Command:

```sh
pdfview /root/test.pdf
```

Observed on AArch64 QEMU after the fixes:

- zathura starts through `/scarlet/system/linux-aarch64/usr/bin/zathura`.
- The `Invalid Syscall number: 148`, `150`, and `216` messages are gone.
- The earlier `xkbcommon` include-path error is gone.
- The earlier libwayland `wl_display_dispatch_queue` assertion is gone.
- zathura stays running for at least 30 seconds after GTK shared-memory setup.
- On RISC-V, `gtk3-demo` no longer trips the earlier bridge panic after the
  `sendmsg` copy fix. Further pointer/click validation should look at the
  forwarded input-event log from the bridge.
- Remaining messages are non-fatal minimal-userland warnings:
  - missing D-Bus machine-id for the settings portal;
  - missing GTK settings files;
  - missing `/root/.cache/gtk-3.0/compose` and user Compose file.

## Next Runtime Check

Boot AArch64 Scarlet and run:

```sh
pdfview /root/test.pdf
```

If this still fails visually, the next investigation should start from the
Wayland bridge/SWS surface commit path and GTK's frame lifecycle, not from
kernel memory management.
