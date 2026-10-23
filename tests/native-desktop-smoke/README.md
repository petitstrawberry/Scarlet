# Minimal native desktop smoke

This harness runs the actual AArch64 kernel, bootstrap init, service manager,
SBUS daemon, SWS, the actual global-menu shell, Files picker service and a desktop application under QEMU TCG.
It uses a small initramfs and a 64 MiB Limine boot image; it does **not** build or
validate the full desktop distribution image. The guest has 2 GiB RAM, two virtual
CPUs, virtio keyboard/tablet/GPU and no network device. Pass `--memory 4G` or
`--memory 8G` explicitly for larger fixtures; the exact RAM setting is recorded
in `commands.json`. Compiler acceptance uses 4 GiB because its diskless
initramfs also retains the complete toolchain and source dependencies in RAM. All documents are generated
fixtures. The default run lasts five minutes and shuts down its own QEMU process.

Prerequisites: the supported Scarlet Rust toolchain (26d00a8 or newer), native
`aarch64-unknown-scarlet` standard library, Python 3, AArch64 QEMU/EDK2 firmware,
`mtools`, and `cargo-scarlet-plugin-limine` from the Scarlet SDK revision pinned in
the repository flake. Keep the plugin and mtools on PATH. Do not download a full
root filesystem to run this test.

Use an isolated scratch directory for every concurrent guest. The runner locks
its output directory before rebuilding the image and refuses a concurrent run.
QEMU sockets and screenshots are private files beneath that directory.

## Build inputs

Run from the repository root inside its supported development environment. Use
the `TARGET_CC` unwrapped Clang fallback described in
[`docs/development/README.md`](../../docs/development/README.md), without overriding
`CC_<target>` globally. `SMOKE` and `CARGO_TARGET_DIR` below are absolute scratch
directories chosen by the operator.

```sh
tests/loader-smoke/build-kernel.sh --output "$SMOKE/kernel"
python3 tests/native-desktop-smoke/build-bootstrap.py --output "$SMOKE/bootstrap"
cargo build --release --manifest-path user/scarlet-ld/Cargo.toml \
  --target aarch64-unknown-scarlet
```

Build the published SGFX Scarlet VirGL plugin, then use that revision's
`platforms/scarlet/native-config.py install` workflow to generate `userspace.toml`
and the driver descriptor. The normal image builder uses this same configuration.
It supplies the `/bin/scarlet-ld` interpreter and dynamic SGFX import policy;
plain Cargo without it fails to link `dlopen`, `dlsym` and `dlerror`. Do not add a
fake loader or globally ignore linker errors. For SGFX revision `bfd5179`, the
script takes three positional arguments:

```sh
python3 "$SGFX_SOURCE/platforms/scarlet/native-config.py" install \
  "$SMOKE_PROJECT" "$SGFX_DRIVER_DIRECTORY"
```

`SMOKE_PROJECT/scarlet.toml` must identify the BSP directory through `[bsp] path`;
that BSP's `.cargo/config.toml` specifies the AArch64 build target. The driver
directory already contains `libsgfx_scarlet_virgl.so`. The script writes
`SMOKE_PROJECT/.scarlet/sgfx/userspace.toml` and the driver descriptor. Use a normal
Scarlet project or a disposable project with those two small configuration files.

```sh
cargo build --release --locked --manifest-path .cargo/Cargo.toml \
  --config "$SGFX_USERSPACE_CONFIG" --target aarch64-unknown-scarlet \
  -p scarlet-std-bin --bin sws --bin stemd --bin logd --bin scarlet-shell --bin files \
  --bin notepad --bin task-manager --bin screenshot
```

The test stages binaries that are already built; it does not silently patch their
dependencies. For local UI work, explicitly add the local patch configuration,
record it in `--build-label`, and restore any temporary lockfile resolution before
publishing. A published-pin run should use `--locked` without local overrides.

## Stage and boot

```sh
python3 tests/native-desktop-smoke/stage.py \
  --bin-dir "$CARGO_TARGET_DIR/aarch64-unknown-scarlet/release" \
  --init "$CARGO_TARGET_DIR/aarch64-unknown-scarlet/release/native-smoke-init" \
  --sbusd "$CARGO_TARGET_DIR/aarch64-unknown-scarlet/release/sbusd" \
  --prepare "$CARGO_TARGET_DIR/aarch64-unknown-scarlet/release/prepare-desktop" \
  --driver "$SGFX_DRIVER" --descriptor "$SGFX_DESCRIPTOR" \
  --assets "$PWD/bundles/desktop/fs/share" --ui-source "$UI_SOURCE" \
  --app notepad --build-label "published revision or local patch description" \
  --output "$SMOKE/staging"

python3 tests/native-desktop-smoke/run.py \
  --kernel "$SMOKE/kernel/target/aarch64-unknown-none-elf/debug/scarlet-loader-smoke-kernel" \
  --staging "$SMOKE/staging" --output "$SMOKE/qemu" \
  --limine-plugin "$LIMINE_PLUGIN" --qemu "$QEMU_AARCH64" \
  --firmware-code "$EDK2_AARCH64_CODE" --firmware-vars "$EDK2_ARM_VARS" \
  --duration 600
```

`stage.py` refuses to overwrite an existing staging directory. It records binary
hashes and source revision/file hashes in `staging-provenance.json` beside that
directory. The runner records kernel/initramfs hashes, exact commands, serial log,
an initial PNG and bounded-run results. A captured screenshot alone is **not** an
automatic functional pass. Inspect the screen and exercise the intended actions.
TCG processing can lag queued input, so wait for the expected visible state before
sending the next action. This setup is not a native performance benchmark.

Examples while the runner is alive:

```sh
python3 tests/native-desktop-smoke/qmp.py "$SMOKE/qemu/control.sock" \
  '{"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":"ctrl"},{"type":"qcode","data":"a"}],"hold-time":150}}'
python3 tests/native-desktop-smoke/qmp.py "$SMOKE/qemu/control.sock" \
  "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$SMOKE/qemu/interaction.png\",\"format\":\"png\"}}"
```

Notepad starts with `/tmp/fixture.txt` on tmpfs. Check select-all, typing, Save,
Find/Replace, Escape, and the Save/Discard/Cancel guard after a new edit. Other
supported initial windows are `files`, `task-manager`, and `screenshot`; Files
still runs as a picker service in every case. `scarlet-shell` is required to
exercise the native global File/Edit/View menu; a guest without it cannot verify
the menu API even if the application window renders correctly.

The fixture setup also logs write/sync/rename probes for tmpfs and the initramfs
overlay root. The initial kernel rejected overlay replacement; the subsequent
bounded driver change supports same-parent regular files staged in the tmpfs
upper layer, including replacement of a lower regular destination. The separate
[overlay save fixture](../native-overlay-save-smoke/README.md) exercises repeated
actual Notepad saves, retained handles and rejected operations. See the
[scope and exclusions](OVERLAY_RENAME_AUDIT.md). Neither tmpfs nor this disposable
overlay fixture proves persistence across reboot.

The last captured native menu run retained the initial global label (`Untitled —
Notepad`) after a file opened, while the client-drawn title updated. Its platform
`set_title` implementation was a no-op and the shell used the initial focus label;
the application catalog was not involved. A subsequent title-propagation fix
uses the existing `SET_WINDOW_TITLE` message, but its final native GUI check was
deferred at the requested Mac handoff. Rebuild both SWS/shell and the application
with the matching UI revision before testing Open, Save As, New and focus changes.
The existing capture verifies visible menus and File → New, not that later fix.
