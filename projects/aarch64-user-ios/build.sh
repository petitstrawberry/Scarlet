#!/bin/bash
set -euo pipefail

project=$(cd "$(dirname "$0")" && pwd)
repo=$(git -C "$project" rev-parse --show-toplevel)
probe_src="$project/tests/native-execution"
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

app_name=Scarlet
display_name=Scarlet
default_id=org.scarletos.ios
framework=""
link_flags=(-framework UIKit -framework Foundation)
if $probe; then
    app_name=ScarletExecutionProbe
    display_name="Scarlet Execution Probe"
    default_id=org.scarletos.ios-p0
    flags+=(-DSCARLET_EXECUTION_PROBE -I "$probe_src")
    framework="$out/$app_name.app/Frameworks/ScarletFixture.framework"
    mkdir -p "$framework"
    "$clang" "${flags[@]}" -dynamiclib -nostdlib \
        -Wl,-install_name,@rpath/ScarletFixture.framework/ScarletFixture \
        "$probe_src/fixture.S" -lSystem -o "$framework/ScarletFixture"
else
    SDKROOT="$sdkroot" IPHONEOS_DEPLOYMENT_TARGET=16.0 \
        cargo build --locked --release -Zbuild-std=std,panic_abort \
        --manifest-path "$project/bsp/Cargo.toml" --target "$rust_target" \
        --target-dir "$build_root/rust" >&2
    implementation=("$build_root/rust/$rust_target/release/libscarlet_aarch64_user_ios_bsp.a")
    link_flags+=(-Wl,-dead_strip -framework Security -framework CoreFoundation)
fi
app="$out/$app_name.app"
bundle_id=${BUNDLE_ID:-"$default_id"}
mkdir -p "$app"
"$clang" "${flags[@]}" -fobjc-arc "$project/app/main.m" "${implementation[@]}" \
    "${link_flags[@]}" -o "$app/$app_name"

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

python3 - "$app" "$framework" "$bundle_id" "$platform" "$out" "$app_name" "$display_name" <<'PY'
from pathlib import Path
import plistlib
import sys

app, framework, bundle_id, platform, build, app_name, display_name = sys.argv[1:]
common = {
    "CFBundleInfoDictionaryVersion": "6.0",
    "CFBundleShortVersionString": "1.0",
    "CFBundleSupportedPlatforms": [platform],
    "CFBundleVersion": "1",
    "MinimumOSVersion": "16.0",
}
bundles = [(app, app_name, "APPL", bundle_id)]
if framework:
    bundles.append((framework, "ScarletFixture", "FMWK", f"{bundle_id}.fixture"))
for directory, name, kind, ident in bundles:
    info = dict(common, CFBundleExecutable=name, CFBundleIdentifier=ident,
                CFBundleName=name, CFBundlePackageType=kind)
    if kind == "APPL":
        info.update(
            CFBundleDisplayName=display_name,
            UIDeviceFamily=[1, 2],
            UILaunchScreen={},
            UIApplicationSceneManifest={
                "UIApplicationSupportsMultipleScenes": False,
                "UISceneConfigurations": {
                    "UIWindowSceneSessionRoleApplication": [
                        {"UISceneDelegateClassName": "ProbeScene"}
                    ]
                },
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
    (Path(build) / "entitlements.plist").write_bytes(plistlib.dumps(entitlements))
    profile_path.unlink()
PY

if [[ -n "$identity" ]]; then
    if [[ -n "$framework" ]]; then codesign --force --sign "$identity" "$framework"; fi
    codesign --force --sign "$identity" --entitlements "$entitlements" "$app"
    codesign --verify --deep --strict "$app"
elif [[ "$sdk" == iphonesimulator ]]; then
    if [[ -n "$framework" ]]; then codesign --force --sign - "$framework"; fi
    codesign --force --sign - "$app"
else
    echo "Built without development provisioning. A device install requires signing." >&2
fi
success=true
printf '%s\n' "$app"
