#!/usr/bin/env python3
"""Build only the native java.base core archives for Scarlet, without make.

Run inside `nix develop` after java.base-java has generated its JNI headers:
  python3 tools/openjdk/core-libs.py --source artifacts/openjdk-native/jdk21u-jdk-21.0.13-9

Sources are copied into a private staging directory so Scarlet header overrides
also win over quoted includes in Unix sources. The upstream tree is read-only.
The output includes archives, per-source diagnostics, compile_commands.json,
and an undefined-symbol audit against the existing VM, libffi and native SDK.
No Java or HotSpot compilation, downloads, or guest execution is performed.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
LIBRARIES = ("java", "verify", "zip", "jimage")
OWNER = "scarlet-openjdk-core-libs-v1\n"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def symbols(nm, path):
    result = subprocess.run([nm, "--format=posix", "--extern-only", str(path)],
                            text=True, capture_output=True, check=True)
    defined, undefined = set(), set()
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) < 2 or len(fields[1]) != 1:
            continue
        name, kind = fields[:2]
        if kind == "U":
            undefined.add(name)
        elif kind not in ("w", "v", "u"):
            defined.add(name)
    return defined, undefined


def generate_headers(source, build, boot_jdk, output):
    """Use javac on actual JDK sources; never synthesize JNI constants/ABIs.

    This bounded bootstrap is useful while the full java.base port is being
    completed. Only native-bearing classes and three changed 21u dependencies
    are compiled, and all temporary class files are discarded.
    """
    base = source / "src/java.base"
    requested = set()
    for platform in ("share", "unix"):
        for library in LIBRARIES:
            for path in (base / platform / "native" / ("lib" + library)).rglob("*"):
                if path.suffix in (".c", ".cpp", ".h"):
                    requested.update(re.findall(
                        r'#include "((?:java_|jdk_)[A-Za-z0-9_]+)\.h"', path.read_text()))
    classes = {}
    for directory in (base / "share/classes", base / "unix/classes",
                      base / "scarlet/classes", build / "support/gensrc/java.base"):
        for path in directory.rglob("*.java"):
            key = str(path.relative_to(directory).with_suffix("")).replace("/", "_")
            classes[key] = path
    selected = set()
    for name in requested:
        candidates = [(key, path) for key, path in classes.items()
                      if name == key or name.startswith(key + "_")]
        if candidates:
            selected.add(max(candidates, key=lambda item: len(item[0]))[1])
    for name in ("java_lang_StringUTF16", "jdk_internal_access_JavaIOAccess",
                 "jdk_internal_reflect_MethodHandleAccessorFactory"):
        selected.add(classes[name])
    target = output / "native-headers"
    with tempfile.TemporaryDirectory(prefix=".header-src-", dir=output) as temporary:
        stage = Path(temporary)
        files = []
        for path in sorted(selected):
            # Package declaration, not an assumption about the source root.
            package = re.search(r"^package ([\w.]+);", path.read_text(), re.MULTILINE)
            if not package:
                raise ValueError(f"Missing package in native-bearing class: {path}")
            dest = stage / package[1].replace(".", "/") / path.name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, dest)
            files.append(dest)
        command = [str(boot_jdk / "bin/javac"), "-J-Duser.language=en",
                   "-implicit:none", "-nowarn", "--patch-module", "java.base=" + str(stage),
                   "-d", str(stage / "classes"), "-h", str(stage / "headers"), *map(str, files)]
        log = output / "header-generation.log"
        with log.open("w") as stream:
            subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT, check=True)
        if target.exists():
            shutil.rmtree(target)
        shutil.copytree(stage / "headers", target)
    (output / "header-sources.json").write_text(json.dumps(
        {str(path): digest(path) for path in sorted(selected)}, indent=2) + "\n")
    print(f"Generated JNI headers from {len(selected)} JDK sources", flush=True)
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--build", type=Path, help="configured build directory")
    header_options = parser.add_mutually_exclusive_group()
    header_options.add_argument("--headers", type=Path, help="generated java.base JNI headers")
    header_options.add_argument("--boot-jdk", type=Path,
                                help="optionally generate native headers with JDK 21 javac")
    parser.add_argument("--provider", type=Path, action="append", default=[],
                        help="additional object/archive for the symbol audit (repeatable)")
    parser.add_argument("--output", type=Path,
                        default=ROOT / "artifacts/openjdk-native/core-libs")
    parser.add_argument("--ffi", type=Path,
                        default=ROOT / "artifacts/openjdk-native/ffi/lib/libffi.a")
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 16:
        parser.error("--jobs must be between 1 and 16")
    source = args.source.resolve()
    build = (args.build or source / "build/scarlet-zero").resolve()
    headers = (args.headers or build / "support/headers/java.base").resolve()
    required = ["RUNTIME", "CC", "CXX", "AR", "NM"]
    missing = ["SCARLET_NATIVE_" + key for key in required
               if not os.environ.get("SCARLET_NATIVE_" + key)]
    if missing:
        parser.error("Run inside nix develop; missing " + ", ".join(missing))
    if not args.boot_jdk and not (headers / "java_lang_System.h").is_file():
        parser.error(f"Generate java.base JNI headers first; missing {headers}/java_lang_System.h")
    runtime = Path(os.environ["SCARLET_NATIVE_RUNTIME"])
    output = args.output.absolute()
    if output.is_symlink():
        parser.error("Output must not be a symlink")
    output.mkdir(parents=True, exist_ok=True)
    marker = output / ".scarlet-core-libs"
    if not marker.exists():
        if any(output.iterdir()):
            parser.error("Output must be empty or owned by this helper")
        marker.write_text(OWNER)
    if marker.is_symlink() or marker.read_text() != OWNER:
        parser.error("Unrecognized output owner")
    with marker.open() as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.boot_jdk:
            headers = generate_headers(source, build, args.boot_jdk.resolve(), output)
        return build_libraries(args, source, build, headers, runtime, output)


def build_libraries(args, source, build, headers, runtime, output):
    base = source / "src/java.base"
    overlay = base / "scarlet/native"
    ar, nm = (os.environ["SCARLET_NATIVE_" + name] for name in ("AR", "NM"))
    manifest = {"source": str(source), "build": str(build), "headers": str(headers),
                "runtime": str(runtime), "archives": {}, "sources": {}, "failures": []}
    commands = []
    with tempfile.TemporaryDirectory(prefix=".compile-", dir=output) as temporary:
        work = Path(temporary)
        drivers = work / "drivers"
        drivers.mkdir()
        for name in ("clang", "clang++"):
            shutil.copyfile(HERE / "cc.py", drivers / name)
            (drivers / name).chmod(0o755)
        # Snapshot headers, avoiding concurrent reads while another agent's
        # javac publishes its output. Missing headers fail individual compiles.
        shutil.copytree(headers, work / "headers")
        for library in LIBRARIES:
            dest = work / ("lib" + library)
            dest.mkdir()
            origins = {}
            for directory in (base / "share/native" / dest.name,
                              base / "unix/native" / dest.name,
                              overlay / dest.name):
                if not directory.is_dir():
                    continue
                for path in sorted(directory.rglob("*")):
                    if path.is_file():
                        relative = path.relative_to(directory)
                        target = dest / relative
                        target.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copyfile(path, target)
                        origins[str(relative)] = {"path": str(path), "sha256": digest(path)}
            manifest["sources"][library] = origins
        includes = [work / "headers", build / "support/modules_include/java.base",
                    build / "support/modules_include/java.base/scarlet",
                    work / "libjava", source / "src/hotspot/share/include",
                    source / "src/hotspot/os/posix/include"]
        common = ["-O2", "-g0", "-fPIC", "-ffunction-sections", "-fdata-sections",
                  "-fno-strict-aliasing", "-fwrapv", "-fsigned-char", "-DNDEBUG",
                  "-DSCARLET=1", "-DSTATIC_BUILD=1", "-D_LP64=1", "-D_LITTLE_ENDIAN",
                  '-DARCHPROPNAME="aarch64"', '-DARCH="aarch64"', "-Wall", "-Wextra",
                  "-Wno-unused-parameter", "-Werror=implicit-function-declaration"]
        logs = output / "logs"
        logs.mkdir(exist_ok=True)
        tasks, objects = [], {name: [] for name in LIBRARIES}
        for library in LIBRARIES:
            directory = work / ("lib" + library)
            for path in sorted(directory.rglob("*")):
                if path.suffix not in (".c", ".cpp"):
                    continue
                relative = path.relative_to(directory)
                obj = work / "objects" / library / relative.with_suffix(".o")
                obj.parent.mkdir(parents=True, exist_ok=True)
                objects[library].append(obj)
                command = [str(drivers / ("clang++" if path.suffix == ".cpp" else "clang")),
                           *common, "-std=c++11" if path.suffix == ".cpp" else "-std=c11",
                           "-DLIBRARY_NAME=" + library]
                if library == "zip":
                    # zlib must use the SDK's POSIX descriptors, not gz* Win32 IO.
                    command += ["-DHAVE_UNISTD_H", "-I" + str(directory / "zlib")]
                if library == "jimage":
                    command += ["-include", str(work / "libjava/scarlet_jimage.h")]
                command += ["-I" + str(item) for item in [directory, *includes]]
                command += ["-c", str(path), "-o", str(obj)]
                log = logs / (library + "-" + str(relative).replace("/", "_") + ".log")
                tasks.append((library, str(relative), command, log))
                commands.append({"directory": str(ROOT), "file": str(path),
                                 "arguments": command,
                                 "original": manifest["sources"][library][str(relative)]["path"]})

        def compile_one(task):
            library, name, command, log = task
            with log.open("w") as stream:
                result = subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT)
            return library, name, result.returncode, log

        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            for library, name, status, log in pool.map(compile_one, tasks):
                if status:
                    manifest["failures"].append({"library": library, "source": name,
                                                 "log": str(log)})
                    print(f"FAIL {library}/{name}: {log}", flush=True)
        failed = {item["library"] for item in manifest["failures"]}
        for library in LIBRARIES:
            if library in failed:
                continue
            archive = work / ("lib" + library + ".a")
            subprocess.run([ar, "rcsD", str(archive), *map(str, objects[library])], check=True)
            target = output / archive.name
            os.replace(archive, target)
            manifest["archives"][library] = {"path": str(target), "sha256": digest(target),
                                             "objects": len(objects[library])}
            print(f"Built {target} ({len(objects[library])} objects)", flush=True)

    (output / "compile_commands.json").write_text(json.dumps(commands, indent=2) + "\n")
    providers = [build / "hotspot/variant-zero/libjvm/objs/libjvm.a", args.ffi.resolve()]
    providers += sorted((runtime / "lib").glob("*.a"))
    providers += [path.resolve() for path in args.provider]
    # OpenJDK installs libjvm.a at different locations across make targets.
    if not providers[0].is_file():
        candidates = sorted(build.glob("**/libjvm.a"))
        if candidates:
            providers[0] = candidates[0]
    defined, undefined = set(), set()
    references = {}
    for item in manifest["archives"].values():
        defs, undefs = symbols(nm, Path(item["path"]))
        defined.update(defs)
        undefined.update(undefs)
        for name in undefs:
            references.setdefault(name, []).append(Path(item["path"]).name)
    manifest["providers"] = []
    for path in providers:
        if path.is_file():
            defined.update(symbols(nm, path)[0])
            manifest["providers"].append(str(path))
    manifest["remaining_link_gaps"] = {name: references[name] for name in sorted(undefined - defined)}
    manifest["complete"] = not manifest["failures"]
    (output / "core-libs.json").write_text(json.dumps(manifest, indent=2) + "\n")
    gaps = manifest["remaining_link_gaps"]
    (output / "remaining-link.txt").write_text(
        "Undefined strong symbols from core archives after VM/ffi/SDK providers:\n" +
        "".join(name + ": " + ", ".join(libs) + "\n" for name, libs in gaps.items()))
    print(f"{len(manifest['failures'])} compile failures; {len(gaps)} remaining core link gaps")
    print(f"Manifest: {output / 'core-libs.json'}")
    return 1 if manifest["failures"] else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
