#!/usr/bin/env python3
"""Prepare pinned, unsigned native networking frameworks; never modify the app.

Usage: prepare_network_frameworks.py --sdk iphoneos|iphonesimulator [--build-dir DIR]
Requires Xcode, Meson >= 1.4, Ninja and pkg-config (PATH or explicit tool options).
Release/arm64, iOS 16.0 minimum. stdout is the absolute manifest.json path.
Copy Frameworks/* into App.app/Frameworks and Resources/* into App.app, then sign
each framework with the app identity before signing the app. Add the executable
rpath @executable_path/Frameworks. No framework needs an entitlement of its own.

The ordinary build directory caches verified downloads and validated results.
Sources and intermediate builds exist only in a TemporaryDirectory. A cache key
includes this helper, all source hashes, tool identities and SDK settings.
--simulator-id UUID additionally runs a signed *copy* of the output on that
Simulator, including on cache hits; it never installs an app or touches a BSP.
"""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile


SOURCES = {
    "slirp": (
        "https://gitlab.freedesktop.org/slirp/libslirp/-/archive/v4.9.3/libslirp-v4.9.3.tar.bz2",
        "c82e22c73bdc3f2c038e538d4f0c9c2166defb2402212d61bb7cb1b530ba952f",
    ),
    "glib": (
        "https://download.gnome.org/sources/glib/2.86/glib-2.86.3.tar.xz",
        "b3211d8d34b9df5dca05787ef0ad5d7ca75dec998b970e1aab0001d229977c65",
    ),
    "pcre2": (
        "https://github.com/PCRE2Project/pcre2/releases/download/pcre2-10.44/pcre2-10.44.tar.bz2",
        "d34f02e113cf7193a1ebf2770d3ac527088d485d4e047ed10e5d217c6ef5de96",
    ),
    "pcre2-meson": (
        "https://wrapdb.mesonbuild.com/v2/pcre2_10.44-2/get_patch",
        "4336d422ee9043847e5e10dbbbd01940d4c9e5027f31ccdc33a7898a1ca94009",
    ),
    # Configuration-only fallback; GObject/libffi is not part of the output.
    "libffi": (
        "https://gitlab.freedesktop.org/gstreamer/meson-ports/libffi/-/archive/83d0cfd00d7d37af4b4349511d29f1f0512621b3/libffi-83d0cfd00d7d37af4b4349511d29f1f0512621b3.tar.gz",
        "ab95eb62c77a0c7613458ed14972d9ba8448bb99de5e7901db7c98a49940c9fc",
    ),
    "intl": (
        "https://codeload.github.com/frida/proxy-libintl/tar.gz/33934de09af6a6627eb44e310a8079df009abdbb",
        "4ab5af85ec6124cb526a15c58bbc0877d241807e30855779808dbf9292d861aa",
    ),
}
FRAMEWORKS = {
    "ScarletSlirp": ("slirp", "libslirp.0.dylib"),
    "ScarletGLib": ("glib", "glib/libglib-2.0.0.dylib"),
    "ScarletIntl": ("glib", "subprojects/proxy-libintl/libintl.8.dylib"),
}
PIPE2_OLD = "  if cc.has_function(f)\n"
PIPE2_NEW = "  if cc.has_function(f) and (f != 'pipe2' or cc.has_header_symbol('unistd.h', 'pipe2'))\n"

# A consumer of the public upstream API: lifecycle + Ethernet ARP round trip.
# No guest code, external network permission, JIT, or upstream process helpers.
SMOKE = r'''
#include <libslirp.h>
#include <arpa/inet.h>
#include <stdio.h>
#include <string.h>
static int replies;
static slirp_ssize_t send_packet(const void *data, size_t len, void *opaque) {
    const unsigned char *p = data;
    if (len >= 42 && p[12] == 8 && p[13] == 6 && p[21] == 2) replies++;
    return len;
}
static void error(const char *msg, void *opaque) { fprintf(stderr, "%s\n", msg); }
static int64_t clock_ns(void *opaque) { return 0; }
static void fd(int n, void *opaque) {}
static void notify(void *opaque) {}
int main(void) {
    if (strcmp(slirp_version_string(), "4.9.3")) return 1;
    SlirpConfig cfg = {0};
    cfg.version = 1; cfg.in_enabled = true;
    inet_pton(AF_INET, "10.0.2.0", &cfg.vnetwork);
    inet_pton(AF_INET, "255.255.255.0", &cfg.vnetmask);
    inet_pton(AF_INET, "10.0.2.2", &cfg.vhost);
    inet_pton(AF_INET, "10.0.2.15", &cfg.vdhcp_start);
    inet_pton(AF_INET, "10.0.2.3", &cfg.vnameserver);
    cfg.if_mtu = 1500; cfg.if_mru = 1500; cfg.disable_host_loopback = true;
    SlirpCb cb = {.send_packet=send_packet, .guest_error=error,
        .clock_get_ns=clock_ns, .register_poll_fd=fd, .unregister_poll_fd=fd,
        .notify=notify};
    Slirp *s = slirp_new(&cfg, &cb, NULL);
    if (!s) return 2;
    unsigned char packet[60] = {0};
    memset(packet, 255, 6); packet[6]=2; packet[11]=1;
    packet[12]=8; packet[13]=6; packet[15]=1; packet[16]=8;
    packet[18]=6; packet[19]=4; packet[21]=1;
    memcpy(packet+22, packet+6, 6);
    inet_pton(AF_INET, "10.0.2.15", packet+28);
    inet_pton(AF_INET, "10.0.2.2", packet+38);
    slirp_input(s, packet, sizeof(packet));
    slirp_cleanup(s);
    if (replies != 1) return 3;
    puts("SCARLET_SLIRP_ARP_PASS 4.9.3");
    return 0;
}
'''


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def run(args, env, log, timeout=None, check=True):
    args = [str(x) for x in args]
    log.write("$ " + shlex.join(args) + "\n")
    log.flush()
    result = subprocess.run(args, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, timeout=timeout)
    log.write(result.stdout + f"\nexit={result.returncode}\n")
    log.flush()
    if check and result.returncode:
        raise RuntimeError(f"Command failed: {shlex.join(args)}\n{result.stdout[-4000:]}")
    return result


def download(name, directory):
    url, expected = SOURCES[name]
    target = directory / (name + "-" + expected)
    if target.exists() and digest(target) == expected:
        return target
    with tempfile.NamedTemporaryFile(dir=directory, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            with urllib.request.urlopen(url, timeout=90) as response:
                shutil.copyfileobj(response, stream)
            stream.close()
            if digest(temporary) != expected:
                raise RuntimeError(f"Source SHA-256 mismatch: {url}")
            os.replace(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)
    return target


def extract(archive, destination, is_zip=False):
    """Reject traversal; create in-tree archive links only after regular files."""
    destination.mkdir(parents=True, exist_ok=True)
    def target(name):
        result = (destination / name).resolve()
        if destination.resolve() not in result.parents and result != destination.resolve():
            raise RuntimeError(f"Unsafe archive member: {name}")
        return result
    if is_zip:
        with zipfile.ZipFile(archive) as source:
            for member in source.infolist():
                path = target(member.filename)
                if (member.external_attr >> 16) & 0o170000 == 0o120000:
                    raise RuntimeError("Archive symlink is unsupported")
                if member.is_dir():
                    path.mkdir(parents=True, exist_ok=True)
                else:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(source.read(member))
    else:
        links = []
        with tarfile.open(archive) as source:
            for member in source:
                path = target(member.name)
                if member.isdir():
                    path.mkdir(parents=True, exist_ok=True)
                elif member.isfile():
                    path.parent.mkdir(parents=True, exist_ok=True)
                    with source.extractfile(member) as stream, path.open("wb") as output:
                        shutil.copyfileobj(stream, output)
                    path.chmod(member.mode & 0o777)
                elif member.issym() or member.islnk():
                    links.append(member)
                else:
                    raise RuntimeError(f"Unsupported archive member: {member.name}")
        for member in links:
            path = target(member.name)
            linked = target(str(Path(member.name).parent / member.linkname)
                            if member.issym() else member.linkname)
            path.parent.mkdir(parents=True, exist_ok=True)
            if member.issym():
                path.symlink_to(os.path.relpath(linked, path.parent))
            else:
                shutil.copy2(linked, path)


def validate(output, sdk, env, log):
    allowed = {f"@rpath/{name}.framework/{name}" for name in FRAMEWORKS}
    for name in FRAMEWORKS:
        binary = output / "Frameworks" / (name + ".framework") / name
        if run(["/usr/bin/lipo", "-archs", binary], env, log).stdout.strip() != "arm64":
            raise RuntimeError(f"Incorrect architecture: {binary}")
        version = run(["/usr/bin/xcrun", "vtool", "-show-build", binary], env, log).stdout
        platform = "IOS" if sdk == "iphoneos" else "IOSSIMULATOR"
        if not re.search(r"platform\s+" + platform + r"\s", version) or not re.search(r"minos\s+16\.0\s", version):
            raise RuntimeError(f"Incorrect deployment platform: {binary}")
        deps = run(["/usr/bin/otool", "-L", binary], env, log).stdout.splitlines()[1:]
        for line in deps:
            dependency = line.strip().split(" (compatibility")[0]
            if dependency not in allowed and not dependency.startswith("/usr/lib/"):
                raise RuntimeError(f"Unpackaged dependency: {dependency}")
        commands = run(["/usr/bin/otool", "-l", binary], env, log).stdout
        if "cmd LC_RPATH" in commands or "cmd LC_CODE_SIGNATURE" in commands:
            raise RuntimeError(f"Expected unsigned framework without build rpaths: {binary}")


def smoke(output, work, sdk, flags, clang, simulator, env, log):
    root = work / "consumer"
    root.mkdir()
    (root / "smoke.c").write_text(SMOKE)
    run([clang, *flags, "-O2", root / "smoke.c",
         "-I" + str(output / "Frameworks/ScarletSlirp.framework/Headers"),
         "-F" + str(output / "Frameworks"), "-framework", "ScarletSlirp",
         "-Wl,-rpath,@executable_path/Frameworks", "-o", root / "smoke"], env, log)
    if simulator:
        if sdk != "iphonesimulator":
            raise RuntimeError("--simulator-id requires --sdk iphonesimulator")
        shutil.copytree(output / "Frameworks", root / "Frameworks")
        for name in ["ScarletIntl", "ScarletGLib", "ScarletSlirp"]:
            framework = root / "Frameworks" / (name + ".framework")
            run(["/usr/bin/codesign", "--force", "--sign", "-", framework], env, log)
            run(["/usr/bin/codesign", "--verify", "--strict", framework], env, log)
        run(["/usr/bin/codesign", "--force", "--sign", "-", root / "smoke"], env, log)
        result = run(["/usr/bin/xcrun", "simctl", "spawn", simulator, root / "smoke"], env, log, timeout=30)
        if "SCARLET_SLIRP_ARP_PASS 4.9.3" not in result.stdout:
            raise RuntimeError("Simulator smoke test did not report success")


def prepare(args, root, env, log):
    tools = {}
    for name in ["meson", "ninja", "pkg_config"]:
        tool = shutil.which(getattr(args, name))
        if not tool:
            raise RuntimeError(f"Missing tool: {getattr(args, name)}")
        tools[name] = str(Path(tool).absolute())
    # Prefer Apple native tools over any inherited Nix compiler wrappers.
    env["PATH"] = ":".join([str(Path(x).parent) for x in tools.values()] + ["/usr/bin", "/bin", "/usr/sbin", "/sbin", os.environ.get("PATH", "")])
    sdk = args.sdk
    def xcrun(*command):
        return run(["/usr/bin/xcrun", "--sdk", sdk, *command], env, log).stdout.strip()
    sdkroot = xcrun("--show-sdk-path")
    clang = xcrun("--find", "clang")
    triple = "arm64-apple-ios16.0" + ("-simulator" if sdk == "iphonesimulator" else "")
    flags = ["-target", triple, "-isysroot", sdkroot]
    identity = {"helper": digest(__file__), "sources": SOURCES, "sdk": sdk,
                "sdkroot": sdkroot, "sdk_build": xcrun("--show-sdk-build-version"),
                "sdk_settings": digest(Path(sdkroot) / "SDKSettings.json"),
                "clang": run([clang, "--version"], env, log).stdout,
                "tools": {key: [value, run([value, "--version"], env, log).stdout] for key, value in tools.items()}}
    key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()[:24]
    result = root / "artifacts" / (sdk + "-" + key)
    manifest = result / "manifest.json"
    with tempfile.TemporaryDirectory(prefix=sdk + "-", dir=root / "work") as temporary:
        work = Path(temporary)
        env["PKG_CONFIG_LIBDIR"] = str(work / "empty")
        if manifest.exists():
            data = json.loads(manifest.read_text())
            hashes = data.get("files", {})
            actual = {str(f.relative_to(result)): digest(f) for f in result.rglob("*") if f.is_file() and f != manifest}
            if hashes and hashes == actual:
                validate(result, sdk, env, log)
                smoke(result, work, sdk, flags, clang, args.simulator_id, env, log)
                print("Using validated framework cache", file=sys.stderr)
                return manifest
        print(f"Building native networking for {sdk}; log: {log.name}", file=sys.stderr)
        archives = {name: download(name, root / "downloads") for name in SOURCES}
        unpacked = work / "sources"
        for name in ["slirp", "glib", "libffi", "intl"]:
            extract(archives[name], unpacked / name)
        sources = {name: next((unpacked / name).iterdir()) for name in ["slirp", "glib", "libffi", "intl"]}
        subprojects = sources["glib"] / "subprojects"
        for name, directory in [("libffi", "libffi"), ("intl", "proxy-libintl")]:
            shutil.move(str(sources[name]), subprojects / directory)
            sources[name] = subprojects / directory
        extract(archives["pcre2"], subprojects)
        extract(archives["pcre2-meson"], subprojects, is_zip=True)
        sources["pcre2"] = subprojects / "pcre2-10.44"
        configure = sources["glib"] / "meson.build"
        original = configure.read_text()
        if original.count(PIPE2_OLD) != 1:
            raise RuntimeError("GLib pipe2 probe context changed")
        configure.write_text(original.replace(PIPE2_OLD, PIPE2_NEW))
        cross = work / "cross.ini"
        cross.write_text("[binaries]\n" + "\n".join(f"{name} = {value!r}" for name, value in {
            "c": clang, "cpp": clang + "++", "objc": clang, "ar": "/usr/bin/ar",
            "strip": "/usr/bin/strip", "pkg-config": tools["pkg_config"]}.items()) +
            "\n[host_machine]\nsystem = 'darwin'\ncpu_family = 'aarch64'\ncpu = 'arm64'\nendian = 'little'\n[properties]\nneeds_exe_wrapper = true\n[built-in options]\n" +
            "\n".join(f"{name} = {flags!r}" for name in ["c_args", "c_link_args", "cpp_args", "cpp_link_args", "objc_args", "objc_link_args"]) + "\n")
        builds = {name: work / (name + "-build") for name in ["glib", "slirp"]}
        common = ["--cross-file", cross, "--buildtype=release", "--wrap-mode=nodownload"]
        glib_options = ["--default-library=both", "-Dtests=false", "-Dintrospection=disabled", "-Dnls=disabled", "-Ddocumentation=false", "-Dman-pages=disabled", "-Dlibmount=disabled", "-Dselinux=disabled", "-Dlibelf=disabled", "-Dsysprof=disabled", "-Ddtrace=disabled", "-Dsystemtap=disabled", "-Dpcre2:jit=disabled"]
        try:
            run([tools["meson"], "setup", builds["glib"], sources["glib"], *common, *glib_options], env, log)
            run([tools["ninja"], "-C", builds["glib"], "-j", str(args.jobs), "glib/libglib-2.0.0.dylib"], env, log)
            env["PKG_CONFIG_LIBDIR"] = str(builds["glib"] / "meson-uninstalled")
            run([tools["meson"], "setup", builds["slirp"], sources["slirp"], *common, "--default-library=shared", "-Dstatic=false"], env, log)
            run([tools["ninja"], "-C", builds["slirp"], "-j", str(args.jobs), "libslirp.0.dylib"], env, log)
        finally:
            for name, build in builds.items():
                meson_log = build / "meson-logs/meson-log.txt"
                if meson_log.exists():
                    shutil.copy2(meson_log, str(log.name) + "." + name + ".txt")
        output = work / "output"
        mapping = {"@rpath/" + Path(path).name: f"@rpath/{name}.framework/{name}" for name, (_, path) in FRAMEWORKS.items()}
        for name, (build, path) in FRAMEWORKS.items():
            framework = output / "Frameworks" / (name + ".framework")
            framework.mkdir(parents=True)
            binary = framework / name
            shutil.copy2(builds[build] / path, binary)
            run(["/usr/bin/install_name_tool", "-id", f"@rpath/{name}.framework/{name}", binary], env, log)
            dependencies = run(["/usr/bin/otool", "-L", binary], env, log).stdout
            for old, new in mapping.items():
                if old in dependencies:
                    run(["/usr/bin/install_name_tool", "-change", old, new, binary], env, log)
            commands = run(["/usr/bin/otool", "-l", binary], env, log).stdout
            for rpath in re.findall(r"cmd LC_RPATH\s+cmdsize \d+\s+path (.*?) \(offset", commands):
                run(["/usr/bin/install_name_tool", "-delete_rpath", rpath, binary], env, log)
            if "cmd LC_CODE_SIGNATURE" in commands:
                run(["/usr/bin/codesign", "--remove-signature", binary], env, log)
            info = {"CFBundleExecutable": name, "CFBundleIdentifier": "org.scarletos.native." + name.lower(), "CFBundleName": name, "CFBundlePackageType": "FMWK", "CFBundleVersion": "1", "CFBundleShortVersionString": "1.0", "MinimumOSVersion": "16.0", "CFBundleSupportedPlatforms": ["iPhoneOS" if sdk == "iphoneos" else "iPhoneSimulator"]}
            (framework / "Info.plist").write_bytes(plistlib.dumps(info))
        headers = output / "Frameworks/ScarletSlirp.framework/Headers"
        headers.mkdir()
        shutil.copy2(sources["slirp"] / "src/libslirp.h", headers)
        shutil.copy2(builds["slirp"] / "libslirp-version.h", headers)
        notices = output / "Resources/ThirdPartyLicenses/ScarletNetworking"
        for name, source in sources.items():
            count = 0
            for item in source.rglob("*"):
                relative = item.relative_to(source)
                if "subprojects" in relative.parts or not item.is_file():
                    continue
                if re.match(r"(?i)^(copying|copyright|licen[cs]e|authors|notice)([.-]|$)", item.name) or any(part.lower() == "licenses" for part in relative.parts):
                    destination = notices / name / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(item, destination)
                    count += 1
            if not count:
                raise RuntimeError(f"No upstream license notices found for {name}")
        (notices / "sources.json").write_text(json.dumps(SOURCES, indent=2) + "\n")
        (notices / "glib-pipe2-probe.patch").write_text("--- a/meson.build\n+++ b/meson.build\n@@ -829,7 +829,7 @@\n endif\n \n foreach f : functions\n-" + PIPE2_OLD + "+" + PIPE2_NEW + "     define = 'HAVE_' + f.underscorify().to_upper()\n     glib_conf.set(define, 1)\n     glib_conf_prefix = glib_conf_prefix + '#define @0@ 1\\n'.format(define)\n")
        validate(output, sdk, env, log)
        smoke(output, work, sdk, flags, clang, args.simulator_id, env, log)
        data = {"schema_version": 1, "sdk": sdk, "target": triple, "configuration": "Release", "identity": identity,
                "frameworks": ["Frameworks/" + name + ".framework" for name in FRAMEWORKS],
                "resources": "Resources", "sign_order": ["ScarletIntl", "ScarletGLib", "ScarletSlirp"],
                "link_flags": ["-F", str(result / "Frameworks"), "-framework", "ScarletSlirp", "-Wl,-rpath,@executable_path/Frameworks"],
                "dlopen_path": "Frameworks/ScarletSlirp.framework/ScarletSlirp",
                "files": {str(f.relative_to(output)): digest(f) for f in output.rglob("*") if f.is_file()}}
        (output / "manifest.json").write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
        if result.exists():
            shutil.rmtree(result)
        output.rename(result)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sdk", choices=["iphoneos", "iphonesimulator"], required=True)
    parser.add_argument("--build-dir", type=Path, default=Path(__file__).resolve().parents[3] / ".build/scarlet-ios/native-network")
    parser.add_argument("--jobs", type=int, default=3)
    parser.add_argument("--meson", default="meson")
    parser.add_argument("--ninja", default="ninja")
    parser.add_argument("--pkg-config", dest="pkg_config", default="pkg-config")
    parser.add_argument("--simulator-id", help="Run ARP smoke test on an already booted Simulator UUID")
    args = parser.parse_args()
    if args.jobs < 1 or (args.simulator_id and args.sdk != "iphonesimulator"):
        parser.error("jobs must be positive; simulator-id requires iphonesimulator")
    root = args.build_dir.expanduser().resolve()
    for name in ["downloads", "artifacts", "work", "logs"]:
        (root / name).mkdir(parents=True, exist_ok=True)
    env = {key: os.environ[key] for key in ["HOME", "TMPDIR", "PATH"] if key in os.environ}
    env.update(LANG="C", LC_ALL="C", PKG_CONFIG_PATH="", PYTHONDONTWRITEBYTECODE="1")
    env["DEVELOPER_DIR"] = os.environ.get("SCARLET_IOS_DEVELOPER_DIR") or subprocess.check_output(["/usr/bin/xcode-select", "-p"], text=True).strip()
    with (root / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        with tempfile.NamedTemporaryFile(mode="w", prefix=args.sdk + "-", suffix=".log", dir=root / "logs", delete=False) as log:
            try:
                manifest = prepare(args, root, env, log)
            except Exception as error:
                print(f"{error}\nBuild log: {log.name}", file=sys.stderr)
                return 1
    print(manifest)
    return 0


if __name__ == "__main__":
    sys.exit(main())
