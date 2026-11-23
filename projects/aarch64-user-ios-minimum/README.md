# Scarlet Minimum on iOS

Scarlet Minimum runs the Base and CLI Utilities bundles in an iOS app. It
boots to a guest console. For the graphical session and bundled desktop apps,
use [Scarlet Desktop on iOS](../aarch64-user-ios-desktop/README.md).

## Run in Simulator

Use an Apple silicon Mac with Xcode and an iOS Simulator runtime. From the
repository root, enter the Scarlet development shell and boot a Simulator.

```sh
projects/aarch64-user-ios-minimum/tools/scarlet_ios.sh run --release --locked
```

The runner builds, installs, and launches `org.scarletos.ios`. Tap the console
to focus input. Sign in as `root` with an empty password on a fresh Minimum
disk. The console's Ctrl button applies to the next character.

To build without launching, use `image --release --locked` in place of
`run --release --locked`. The project writes its filesystem images and an app
path file under `.scarlet/images/`. First launch copies the bundled ext2 image
to Application Support as `scarlet-root.ext2`. Later launches and app updates
reuse that writable disk.

## Install on an iPhone or iPad

Pair the device with Xcode, enable Developer Mode, and unlock it during
installation. Use an Apple Development identity and a provisioning profile
that includes the device and covers `org.scarletos.ios`. [Apple's device setup
guide](https://developer.apple.com/documentation/xcode/running-your-app-on-simulated-or-physical-devices)

Set `device` to the identifier printed by `xcrun devicectl list devices`. Run
these commands from the repository root in the development shell.

```sh
project=projects/aarch64-user-ios-minimum
export PROVISIONING_PROFILE=/path/to/profile.mobileprovision
export SIGNING_IDENTITY='Apple Development: Name (TEAMID)'
"$project/tools/scarlet_ios.sh" image --target aarch64-apple-ios --release --locked

app=$(cat "$project/.scarlet/images/iphoneos-release.app-path")
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun devicectl device install app --device "$device" "$app"
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun devicectl device process launch \
  --device "$device" --console org.scarletos.ios
```

Use the same bundle identifier for packaging, provisioning, and launch if
you set `BUNDLE_ID`.

## Smoke tests

Build and install the Simulator app first. Use a disposable Simulator for
`--boot-check`, which writes a marker to the guest disk.

| Launch argument | Checks | Expected result |
| --- | --- | --- |
| `--boot-check` | Guest boot, signed shared-library loading, and a disk marker read on the next boot | `SCARLET_BOOT_PASS` |
| `--scheduler-check` | Guest scheduling, signed exec, process groups, and heap growth and shrinkage | `SCARLET_AOT_PASS` |

```sh
SCARLET_RELEASE=1 projects/aarch64-user-ios-minimum/tools/run_ios.sh --scheduler-check
```

`tools/test_runtime_link.py` exercises dependency selection, `dlopen`, and
catalog rejection in a separate Simulator app. Pass the path from
`iphonesimulator-release.app-path`.

## How the guest runs

`scarlet.toml` defines the BSP, initramfs, and ext2 image. The build wrapper
selects an AArch64 guest toolchain that reserves x18 and uses 16 KiB load
alignment. Packaging converts staged ELF executables and shared libraries to
signed frameworks. At exec or library load, Scarlet looks up the guest file
by content hash in the app's catalog. Install an updated app package to run
guest binaries that introduce new hashes.
