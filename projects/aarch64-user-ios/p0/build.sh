#!/bin/bash
set -euo pipefail

src=$(cd "$(dirname "$0")" && pwd)
repo=$(git -C "$src" rev-parse --show-toplevel)
sdk=${1:-iphoneos}
case "$sdk" in
    iphoneos) target=arm64-apple-ios16.0; platform=iPhoneOS ;;
    iphonesimulator) target=arm64-apple-ios16.0-simulator; platform=iPhoneSimulator ;;
    macosx) target=arm64-apple-macos13.0; platform=MacOSX ;;
    *) echo "usage: $0 [iphoneos|iphonesimulator|macosx] [output-directory]" >&2; exit 2 ;;
esac

output=${2:-"$repo/.build/scarlet-ios-p0/$sdk"}
mkdir -p "$output"
out=$(mktemp -d "$(cd "$output" && pwd)/build.XXXXXXXX")
success=false
trap 'if ! $success; then rm -rf -- "$out"; fi' EXIT
clang=$(xcrun --sdk "$sdk" --find clang)
flags=(-target "$target" -isysroot "$(xcrun --sdk "$sdk" --show-sdk-path)" -O2 -DNDEBUG -Wall -Wextra -Werror)
implementation=("$src/probe.c" "$src/image_pool.c" "$src/carrier.c" "$src/carrier_entry.S")

if [[ "$sdk" == macosx ]]; then
    fixture="$out/ScarletFixture.dylib"
    "$clang" "${flags[@]}" -dynamiclib -nostdlib -Wl,-install_name,@rpath/ScarletFixture.dylib \
        "$src/fixture.S" -lSystem -o "$fixture"
    "$clang" "${flags[@]}" "${implementation[@]}" "$src/main.c" -o "$out/scarlet-p0"
    codesign --force --sign - "$fixture"
    codesign --force --sign - "$out/scarlet-p0"
    success=true
    printf '%s\n' "$out"
    exit
fi

app="$out/ScarletP0.app"
framework="$app/Frameworks/ScarletFixture.framework"
bundle_id=${BUNDLE_ID:-org.scarletos.ios-p0}
mkdir -p "$framework"
"$clang" "${flags[@]}" -dynamiclib -nostdlib \
    -Wl,-install_name,@rpath/ScarletFixture.framework/ScarletFixture \
    "$src/fixture.S" -lSystem -o "$framework/ScarletFixture"
"$clang" "${flags[@]}" -fobjc-arc "${implementation[@]}" "$src/main.m" \
    -framework UIKit -framework Foundation -o "$app/ScarletP0"

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

python3 - "$app" "$framework" "$bundle_id" "$platform" "$out" <<'PY'
from pathlib import Path
import plistlib
import sys

app, framework, bundle_id, platform, build = sys.argv[1:]
common = {
    "CFBundleInfoDictionaryVersion": "6.0",
    "CFBundleShortVersionString": "1.0",
    "CFBundleSupportedPlatforms": [platform],
    "CFBundleVersion": "1",
    "MinimumOSVersion": "16.0",
}
for directory, name, kind, ident in (
    (app, "ScarletP0", "APPL", bundle_id),
    (framework, "ScarletFixture", "FMWK", f"{bundle_id}.fixture"),
):
    info = dict(common, CFBundleExecutable=name, CFBundleIdentifier=ident,
                CFBundleName=name, CFBundlePackageType=kind)
    if kind == "APPL":
        info.update(
            CFBundleDisplayName="Scarlet P0",
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
    codesign --force --sign "$identity" "$framework"
    codesign --force --sign "$identity" --entitlements "$entitlements" "$app"
    codesign --verify --deep --strict "$app"
elif [[ "$sdk" == iphonesimulator ]]; then
    codesign --force --sign - "$framework"
    codesign --force --sign - "$app"
else
    echo "Built without development provisioning. A device install requires signing." >&2
fi
success=true
printf '%s\n' "$app"
