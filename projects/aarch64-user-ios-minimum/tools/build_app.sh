#!/bin/bash
set -euo pipefail

project=$(cd "$(dirname "$0")/.." && pwd)
image_project=${SCARLET_IOS_PROJECT:-$project}
repo=$(git -C "$project" rev-parse --show-toplevel)
probe_src="$project/bsp/tests/native-execution"
sdk=${1:-iphonesimulator}
if [[ $# -gt 0 ]]; then shift; fi
probe=false
if [[ "${1:-}" == --probe ]]; then probe=true; shift; fi
case "$sdk" in
    iphoneos) target=arm64-apple-ios16.0; rust_target=aarch64-apple-ios; platform=iPhoneOS ;;
    iphonesimulator) target=arm64-apple-ios16.0-simulator; rust_target=aarch64-apple-ios-sim; platform=iPhoneSimulator ;;
    macosx) target=arm64-apple-macos13.0; platform=MacOSX; probe=true ;;
    *) echo "usage: $0 [iphoneos|iphonesimulator|macosx] [--probe] [output-directory]" >&2; exit 2 ;;
esac

build_root="$repo/.build/scarlet-ios"
output=${1:-"$build_root/$sdk"}
mkdir -p "$output"
out=$(mktemp -d "$(cd "$output" && pwd)/build.XXXXXXXX")
success=false
trap 'if ! $success; then rm -rf -- "$out"; fi' EXIT
export DEVELOPER_DIR=${SCARLET_IOS_DEVELOPER_DIR:-$(env -u DEVELOPER_DIR /usr/bin/xcode-select -p)}
unset SDKROOT
clang=$(/usr/bin/xcrun --sdk "$sdk" --find clang)
sdkroot=$(/usr/bin/xcrun --sdk "$sdk" --show-sdk-path)
flags=(-target "$target" -isysroot "$sdkroot" -O2 -DNDEBUG -Wall -Wextra -Werror)
implementation=("$probe_src/probe.c" "$probe_src/image_pool.c" "$probe_src/carrier.c" "$probe_src/carrier_entry.S")

if [[ "$sdk" == macosx ]]; then
    fixture="$out/ScarletFixture.dylib"
    "$clang" "${flags[@]}" -dynamiclib -nostdlib -Wl,-install_name,@rpath/ScarletFixture.dylib \
        "$probe_src/fixture.S" -lSystem -o "$fixture"
    "$clang" "${flags[@]}" "${implementation[@]}" "$probe_src/main.c" -o "$out/scarlet-execution-probe"
    codesign --force --sign - "$fixture"
    codesign --force --sign - "$out/scarlet-execution-probe"
    success=true
    printf '%s\n' "$out"
    exit
fi

if $probe; then
    app_name=ScarletExecutionProbe
    display_name="Scarlet Execution Probe"
    default_id=org.scarletos.ios-p0
else
    app_name=Scarlet
    display_name=${SCARLET_IOS_DISPLAY_NAME:-Scarlet}
    default_id=org.scarletos.ios
fi
app="$out/$app_name.app"
mkdir -p "$app"
if ! $probe; then
    cp "$project/bsp/PrivacyInfo.xcprivacy" "$app/"
fi

build_fixture() {
    local source=$1 entry=$2 elf=$3
    "$clang" -target aarch64-none-elf -c "$project/bsp/tests/$source.S" -o "$out/$source.o"
    ld.lld -pie --no-dynamic-linker -z max-page-size=16384 -z separate-loadable-segments \
        -e "$entry" "$out/$source.o" -o "$elf"
    # Mark the freestanding ELF with Scarlet's OS ABI.
    printf '\123' | dd of="$elf" bs=1 seek=7 conv=notrunc 2>/dev/null
}

package_guest() {
    local elf=$1 name=$2
    shift 2
    printf '%s\n' "$name" >> "$app/guest-images"
    local image="$out/images/$name" framework="$app/Frameworks/$name.framework"
    mkdir -p "$image" "$framework"
    cargo run --quiet --locked --manifest-path "$repo/tools/ios-aot/Cargo.toml" \
        --target-dir "$build_root/rust" -- "$elf" "$image" "$@"
    (
        cd "$image"
        "$clang" "${flags[@]}" -dynamiclib -nostdlib \
            "-Wl,-install_name,@rpath/$name.framework/$name" \
            image.S -lSystem -o "$framework/$name"
    )
}

if $probe; then
    framework="$app/Frameworks/ScarletFixture.framework"
    mkdir -p "$framework"
    "$clang" "${flags[@]}" -dynamiclib -nostdlib \
        -Wl,-install_name,@rpath/ScarletFixture.framework/ScarletFixture \
        "$probe_src/fixture.S" -lSystem -o "$framework/ScarletFixture"
else
    build_fixture scheduler_fixture _scarlet_scheduler_start "$app/scheduler.elf"
    package_guest "$app/scheduler.elf" ScarletGuest

    staging="$image_project/.scarlet/staging"
    initramfs=$(python3 "$project/tools/image_config.py" "$image_project" initramfs)
    rootfs=$(python3 "$project/tools/image_config.py" "$image_project" rootfs)
    executable=$(python3 "$project/tools/image_config.py" "$image_project" executable \
        "$rust_target" "${SCARLET_PROFILE:-release}")
    catalog_options=("$staging/initramfs" "$staging/rootfs")
    if [[ -f "$image_project/ios-catalog-exclusions.json" ]]; then
        catalog_options+=(--exclusions "$image_project/ios-catalog-exclusions.json")
    fi
    python3 "$project/tools/catalog_objects.py" "${catalog_options[@]}" > "$out/catalog"
    while IFS= read -r -d '' mode && IFS= read -r -d '' digest && IFS= read -r -d '' elf; do
        if [[ -n "$mode" ]]; then
            package_guest "$elf" "ScarletImage$digest" "$mode"
        else
            package_guest "$elf" "ScarletImage$digest"
        fi
    done < "$out/catalog"
    cp "$initramfs" "$app/initramfs.cpio"
    cp "$rootfs" "$app/root.ext2"
    cp "$executable" "$app/$app_name"
fi
bundle_id=${BUNDLE_ID:-"$default_id"}
if $probe; then
    objects=()
    for source in "${implementation[@]}"; do
        object="$out/$(basename "$source").o"
        "$clang" "${flags[@]}" -c "$source" -o "$object"
        objects+=("$object")
    done
    /usr/bin/xcrun --sdk "$sdk" swiftc -target "$target" -sdk "$sdkroot" \
        -swift-version 6 -warnings-as-errors -O -parse-as-library -D SCARLET_EXECUTION_PROBE \
        -import-objc-header "$probe_src/probe.h" "$project/bsp/src/ScarletApp.swift" \
        "$project/bsp/src/TerminalView.swift" \
        "${objects[@]}" -o "$app/$app_name"
fi

entitlements="$out/entitlements.plist"
profile=${PROVISIONING_PROFILE:-}
identity=${SIGNING_IDENTITY:-}
if [[ -n "$profile" || -n "$identity" ]]; then
    if [[ "$sdk" != iphoneos || ! -f "$profile" || -z "$identity" ]]; then
        echo "Device signing needs PROVISIONING_PROFILE and SIGNING_IDENTITY with iphoneos." >&2
        exit 2
    fi
    security cms -D -i "$profile" -o "$out/profile.plist"
    cp "$profile" "$app/embedded.mobileprovision"
fi

python3 - "$app" "$bundle_id" "$platform" "$out" "$app_name" "$display_name" <<'PY'
from pathlib import Path
import os
import plistlib
import sys

app, bundle_id, platform, build, app_name, display_name = sys.argv[1:]
common = {
    "CFBundleInfoDictionaryVersion": "6.0",
    "CFBundleShortVersionString": "1.0",
    "CFBundleSupportedPlatforms": [platform],
    "CFBundleVersion": "1",
    "MinimumOSVersion": "16.0",
}
bundles = [(app, app_name, "APPL", bundle_id)]
for framework in (Path(app) / "Frameworks").glob("*.framework"):
    name = framework.stem
    bundles.append((framework, name, "FMWK", f"{bundle_id}.{name.lower()}"))
for directory, name, kind, ident in bundles:
    info = dict(common, CFBundleExecutable=name, CFBundleIdentifier=ident,
                CFBundleName=name, CFBundlePackageType=kind)
    if kind == "APPL":
        info.update(
            CFBundleDisplayName=display_name,
            CFBundleDevelopmentRegion="en",
            ScarletDefaultDisplay=os.environ.get("SCARLET_IOS_DEFAULT_DISPLAY", "0") == "1",
            UIDeviceFamily=[1, 2],
            UILaunchScreen={},
            UIApplicationSceneManifest={
                "UIApplicationSupportsMultipleScenes": False,
            },
        )
    (Path(directory) / "Info.plist").write_bytes(plistlib.dumps(info))

profile_path = Path(build) / "profile.plist"
if profile_path.exists():
    profile = plistlib.loads(profile_path.read_bytes())
    source = profile["Entitlements"]
    prefix, allowed = source["application-identifier"].split(".", 1)
    if not (allowed == bundle_id or (allowed.endswith("*") and bundle_id.startswith(allowed[:-1]))):
        sys.exit(f"Provisioning profile does not cover {bundle_id}")
    entitlements = {
        "application-identifier": f"{prefix}.{bundle_id}",
        "com.apple.developer.team-identifier": source["com.apple.developer.team-identifier"],
    }
    # Signed guests reserve separate address windows inside the native app.
    # Request the larger address space when the selected profile grants it.
    extended_va = "com.apple.developer.kernel.extended-virtual-addressing"
    if source.get(extended_va) is True:
        entitlements[extended_va] = True
    (Path(build) / "entitlements.plist").write_bytes(plistlib.dumps(entitlements))
    profile_path.unlink()
PY

sign_frameworks() {
    local framework
    for framework in "$app"/Frameworks/*.framework; do
        [[ -d "$framework" ]] || continue
        codesign --force --sign "$1" "$framework"
    done
}

if [[ -n "$identity" ]]; then
    sign_frameworks "$identity"
    codesign --force --sign "$identity" --entitlements "$entitlements" "$app"
    codesign --verify --deep --strict "$app"
elif [[ "$sdk" == iphonesimulator ]]; then
    sign_frameworks -
    codesign --force --sign - "$app"
else
    echo "Built without development provisioning. A device install requires signing." >&2
fi
success=true
printf '%s\n' "$app"
