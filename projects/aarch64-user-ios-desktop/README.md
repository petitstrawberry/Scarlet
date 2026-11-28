# Scarlet Desktop on iOS

This project packages the stock `bundles/desktop` distribution with the iOS
Minimum project's BSP, initramfs fixtures, guest sysroot and compiler wrappers.
Its userspace config includes the shared iOS ABI flags and SGFX's generated
native-link config, prepared by the standard SGFX bundle in both images.
Minimum remains the Base + CLI distribution. There is no separate Desktop BSP
or replacement guest desktop implementation.
Both filesystem copies of `/init` use the existing `ios-init-pie` feature so
the catalog never receives Base's fixed-address bootstrap executable.

From the repository root in the Scarlet development shell:

```sh
projects/aarch64-user-ios-desktop/tools/scarlet_ios.sh image --release --locked
projects/aarch64-user-ios-desktop/tools/scarlet_ios.sh run --release --locked -- --display --tablet
```

The shared Apple BSP currently pins SGFX's WGPU backend at
`c373416e8dc3d86e7ff1bc397d6b5383aa093409`, which provides shared color-target
and sampled-image bindings. GitHub currently does not serve that revision.
The SDK's project-local Cargo Git cache needs the revision from a trusted
local SGFX checkout. Both Apple BSPs use this same pin.

iPhone and iPad report tablet posture by default. `--tablet` can also force it
when running the iOS app on other device idioms.
The runner installs on the booted Simulator. To package for a device, use
`image --target aarch64-apple-ios --release --locked` with the existing
`PROVISIONING_PROFILE` and `SIGNING_IDENTITY` settings. The default bundle ID is
`org.scarletos.ios.desktop`; `BUNDLE_ID` overrides it for provisioning.
Use a device provisioning profile that grants Extended Virtual Addressing
for Desktop. Individual signed guests reserve host virtual address windows.
The packager includes the entitlement when the selected profile grants it.
Desktop has a separate app container from Minimum, preserving its CLI disk.
Its Info.plist sets the boolean `ScarletDefaultDisplay` to `true` for the Swift
launch UI; Minimum defaults to `false`. The shared packager accepts
`SCARLET_IOS_DEFAULT_DISPLAY=1`, which the Desktop packaging hook supplies.
The hook also sets `SCARLET_IOS_DISPLAY_NAME="Scarlet Desktop"` for the Home
screen label; Minimum keeps `Scarlet`.
As with Minimum, an existing Desktop writable root is retained across installs.

The image outputs and `iphonesimulator-release.app-path` live in this project's
`.scarlet/images/`. To run an already packaged app without rebuilding:

```sh
SCARLET_RELEASE=1 projects/aarch64-user-ios-desktop/tools/run_ios.sh --display --tablet
```

The rootfs contains the existing macOS guest driver at
`/lib/sgfx/libsgfx_hosted.so` and its unchanged `hosted-wgpu.sgfx-driver`
descriptor (`gpu_backend=scarlet-hosted`). The SDK builds that source for
`aarch64-unknown-scarlet` through the shared x18-reserving Rust wrapper and
16 KiB segment flags. Source-owned C/C++ recipes use the shared `-ffixed-x18`
wrappers. No macOS host library is linked into the iOS app.
Source-owned Cargo recipes default to the shared iOS BSP's target directory,
keeping their rebuilt-sysroot artifacts separate from the ordinary userspace
build cache. An explicit `CARGO_TARGET_DIR` is preserved; the wrapper resolves
relative overrides against the caller's working directory before invoking the
SDK. Packaging reads the BSP executable from that same directory, target and
profile. Use the same override when repackaging directly. SDK-managed layers
retain their project-local caches.
The pinned SDK still checks the canonical BSP `target` path unless given
`image --kernel-elf`. For custom target directories the wrapper supplies that
argument. A custom-directory `run` performs this image step, then invokes
`run --no-image`, preserving the application arguments. Default-directory
commands and explicit `run --no-image` retain their normal SDK paths.

Packaging scans both staged filesystems, converts each unique ELF to a signed
framework and lists it in `guest-images`. This includes the hosted DSO and the
stock Desktop's other libraries. The runtime's existing digest-based trusted
catalog resolves the guest DSO; packaging does not bypass its checks. The shared
iOS BSP supplies the hosted SGFX GPU endpoint and Metal presentation, using the
shared hosted backend. This project packages its stock guest-side driver and
Desktop distribution.

The stock Myrica browser, Myrica Vulkan probe and Kureha Code prebuilts remain
on disk but cannot execute on iOS: the first two are fixed-address ELF files,
and Kureha lacks 16 KiB load-segment alignment. These are the same prebuilt
exceptions in the earlier validated stock image. `ios-catalog-exclusions.json`
records their exact hashes and reasons; changed or missing files require review.
Every other catalog/AOT failure remains fatal. No executable framework is
registered for these three programs. Compatible upstream prebuilts are needed
before they can join the signed catalog.

The shared packaging API selects the image project with `SCARLET_IOS_PROJECT`;
the SDK post-image hook supplies `SCARLET_PROJECT_DIR`, `SCARLET_TARGET_TRIPLE`
and `SCARLET_PROFILE`. BSP and image output paths are read from `scarlet.toml`.
To repackage already staged images and an existing BSP executable:

```sh
SCARLET_TARGET_TRIPLE=aarch64-apple-ios-sim SCARLET_PROFILE=release \
  projects/aarch64-user-ios-desktop/tools/package_image.sh
python3 projects/aarch64-user-ios-desktop/tools/verify_package.py
```

The verifier checks stock Desktop entrypoints, the driver descriptor, the
hash-derived framework catalog, copied filesystem images and app signatures.
It does not assert GPU rendering or guest service startup.
