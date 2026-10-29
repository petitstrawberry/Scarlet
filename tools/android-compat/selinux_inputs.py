"""Build identity for the stock libselinux / Shield authorization probe."""
import argparse
import json
from pathlib import Path
import platform
import shutil
import subprocess
from art_inputs import elf_needed, sha256, verify_art_runtime

SOURCE = Path(__file__).resolve().parent
LIBRARY = "system/lib64/libselinux.so"


def verify_selinux_build(build, runtime, transitions=False):
    verify_art_runtime(runtime)
    metadata = json.loads((build / "build.json").read_text())
    binary = build / "selinux-probe"
    expected = {"result": "BUILT", "kind": "stock-label-transition-v1" if transitions else "stock-selinux-probe-v1",
                "source_sha256": sha256(SOURCE / ("label-transition-probe.c" if transitions else "selinux-probe.c")),
                "runtime_manifest_sha256": sha256(runtime / "art-runtime.json"),
                "libselinux_sha256": sha256(runtime / LIBRARY),
                "binary_sha256": sha256(binary)}
    if any(metadata.get(key) != value for key, value in expected.items()):
        raise ValueError("stale or incomplete libselinux probe build")
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ndk", type=Path, required=True)
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--transitions", action="store_true", help="build the conditional label transition probe")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    runtime, output = args.runtime_root.resolve(), args.output.resolve()
    verify_art_runtime(runtime)
    identity = sha256(runtime / "art-runtime.json"), sha256(runtime / LIBRARY)
    host = {"Darwin": "darwin-x86_64", "Linux": "linux-x86_64"}.get(platform.system())
    compiler = args.ndk.resolve() / "toolchains/llvm/prebuilt" / str(host) / "bin/aarch64-linux-android33-clang"
    if not compiler.is_file():
        parser.error(f"missing NDK compiler: {compiler}")
    output.mkdir(parents=True, exist_ok=False)
    (output / "build.json").write_text('{"result":"PREPARING"}\n')
    source, binary = output / ("label-transition-probe.c" if args.transitions else "selinux-probe.c"), output / "selinux-probe"
    shutil.copy2(SOURCE / source.name, source)
    command = [str(compiler), "-std=c11", "-D_GNU_SOURCE", "-O2", "-g", "-Wall", "-Wextra", "-Werror",
               str(source), "-ldl", "-o", str(binary)]
    subprocess.run(command, check=True)
    verify_art_runtime(runtime)
    if identity != (sha256(runtime / "art-runtime.json"), sha256(runtime / LIBRARY)):
        raise ValueError("runtime changed during build")
    metadata = {"result": "BUILT", "kind": "stock-label-transition-v1" if args.transitions else "stock-selinux-probe-v1", "api": 33, "command": command,
                "ndk": (args.ndk / "source.properties").read_text(),
                "source_sha256": sha256(source), "runtime_manifest_sha256": identity[0],
                "libselinux_sha256": identity[1], "binary_sha256": sha256(binary), "needed": elf_needed(binary)}
    (output / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(output)
