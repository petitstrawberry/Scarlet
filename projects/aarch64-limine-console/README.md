# AArch64 console desktop

The full AArch64 Limine distribution with ScarletShell starting in console mode.
It uses the same application catalog, wallpaper settings, desktop services,
and kernel configuration as `aarch64-limine-full`. Console sessions use SWS's
Focused windowing with a single application scene per workspace, while
retaining the real hardware posture and the previous desktop policy.
A final rootfs layer selects
`scarlet-desktop --shell-mode console`; the supervisor preserves the mode on restart.

From the repository's Nix development environment:

```sh
cargo make run-aarch64-console
# Build images without starting QEMU:
cargo make image-aarch64-console
```

To build and run directly from this project directory:

```sh
cd projects/aarch64-limine-console
cargo scarlet image --project . --release
cargo scarlet run --project . --release
```

This project's kernel modules, lock file, and images are isolated under its own
directory. Its thin runner uses the full distribution runner with console image
paths, retaining the existing QEMU acceleration, display, audio, input, and
snapshot options. See [QEMU configuration](../../docs/development/qemu.md) and
[console shell controls](../../docs/desktop/console-shell.md).

The current console work also uses changes in the sibling ScarletUI checkout.
This development workspace has an ignored local Cargo patch configuration in
`.scarlet/cache/cargo-home/config.toml` for the UI and SWS crates. A fresh
checkout needs matching local patches until those changes are available in the
pinned ScarletUI dependency. See the console shell build notes linked above.
This includes synchronizing client-managed window sizes with their rendering
pipelines, so the floating workspace buttons remain intact after output resize.
The SWS build also omits empty GPU load passes from backdrop composition,
avoiding an unintended CPU fallback when console materials are displayed.
ScarletUI retains unchanged artwork and layout during selection changes and
re-presents suspended surfaces on resume, so Home appears immediately when
returning from an app. Recently Used and Library keep independent selection
and horizontal scroll positions.

The default desktop display scale is retained. Layout responds to logical output
size: with 200% scaling, a 2560×1440 output provides a 1280×720 console layout.
Use the existing Display settings to choose the scale for your screen.

Use the release configuration above for guest testing. An unoptimized AArch64
userland build stalled during initial SWS communication in the test environment;
debug startup remains unverified.
