#!/usr/bin/env python3
"""Bootstrap the native AArch64 Scarlet port inside the repository's Nix dev shell.

Examples (use the host JDK major selected by sources.json):
  python3 tools/openjdk/build.py --boot-jdk /path/to/jdk-25 --configure-only
  python3 tools/openjdk/build.py --boot-jdk /path/to/jdk-25 --target hotspot
  python3 tools/openjdk/build.py --boot-jdk /path/to/jdk-25 --target java
  python3 tools/openjdk/build.py --target runtime-probe
  python3 tools/openjdk/build.py --target runtime-support-probe

The java target links the native launcher and packages Scarlet java.base into
image/bin/java and image/usr/lib/jvm/openjdk-<major>. The hotspot target builds only
the VM archive. Builds and ELF audits do not validate execution in the guest.
The runtime probe targets compile and audit prerequisites; they never execute them.
runtime-support-probe tests environment setup and C++ TLS destruction when run
in the guest, and needs neither libffi nor OpenJDK sources. Use the separate
tests/run-runtime.py for guest validation.

Output must be empty or owned by this driver. Downloads are SHA-256 checked on
every use. Sources and build directories are keyed by their inputs; changing a
patch, overlay, wrapper, probe source, toolchain or boot JDK creates a new
directory. OpenJDK archives are treated as already ported and used as-is.
Only an explicit sources.openjdk.ported=false enables the legacy local patch
series and overlay; these files are not needed for normal fork builds.
Existing source edits are rejected, never reset. Old builds are retained for inspection.
Each successful invocation records its artifact paths in <output>/<target>.json.
"""

import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.request


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
OWNER = {"owner": "scarlet-openjdk-bootstrap", "format": 1}
NATIVE_VARS = tuple("SCARLET_NATIVE_" + name for name in
                    ("RUNTIME", "CC", "CXX", "AR", "NM", "RANLIB"))
# These can affect configure/compiler discovery or flags. Recording only this
# allowlist avoids leaking unrelated shell variables into build receipts.
BUILD_ENV_VARS = ("PATH", "CC", "CXX", "AR", "NM", "RANLIB", "CFLAGS", "CXXFLAGS",
                  "CPPFLAGS", "LDFLAGS", "LIBS", "CPATH", "C_INCLUDE_PATH",
                  "CPLUS_INCLUDE_PATH", "LIBRARY_PATH", "SDKROOT",
                  "MACOSX_DEPLOYMENT_TARGET", "SOURCE_DATE_EPOCH", "MAKEFLAGS")
FFI_CFLAGS = "-ffunction-sections -fdata-sections -O2 -fPIC -fno-stack-protector"
JVM_FEATURES = ("serialgc,-epsilongc,-g1gc,-parallelgc,-shenandoahgc,-jvmti,"
                "-jfr,-management,-services,-vm-structs,-cds")
CORE_ARCHIVES = ("libjava.a", "libverify.a", "libzip.a", "libjimage.a")
JAVA_INPUTS = ("runtime-support.c", "runtime-bridge.rs", "launcher.c",
               "builtin-symbols.py", "core-libs.py", "image.py")


class BuildError(Exception):
    pass


def log(message):
    print(f"[openjdk] {message}", flush=True)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def identity(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def exists(path):
    return path.exists() or path.is_symlink()


def directory(path):
    if path.is_symlink() or (path.exists() and not path.is_dir()):
        raise BuildError(f"Refusing non-directory or symlink: {path}")
    path.mkdir(exist_ok=True)
    return path


def regular_file(path):
    if not path.is_file() or path.is_symlink():
        raise BuildError(f"Expected a regular file: {path}")
    return path


def write_once(path, data, executable=False):
    if exists(path):
        if regular_file(path).read_bytes() != data:
            raise BuildError(f"Refusing to overwrite modified input: {path}")
        if executable and not os.access(path, os.X_OK):
            raise BuildError(f"Compiler driver is no longer executable: {path}")
        return
    with path.open("xb") as stream:
        stream.write(data)
    path.chmod(0o755 if executable else 0o644)


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def write_json(path, value):
    if exists(path):
        regular_file(path)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            stream.write(json_bytes(value))
            stream.close()
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


@contextmanager
def output_lock(output, owner=OWNER):
    if output.is_symlink():
        raise BuildError(f"Refusing symlink output directory: {output}")
    output.mkdir(parents=True, exist_ok=True)
    marker = output / ".scarlet-openjdk.json"
    if not exists(marker):
        if any(output.iterdir()):
            raise BuildError(f"Output is not owned by this driver: {output}; choose an empty directory")
        write_once(marker, json_bytes(owner))
    elif json.loads(regular_file(marker).read_text()) != owner:
        raise BuildError(f"Unrecognized output ownership marker: {marker}")
    # Lock the permanent ownership file, so two invocations cannot publish or
    # configure the same tree concurrently. Never unlink a held lock file.
    with marker.open("rb") as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise BuildError(f"Another driver is using {output}") from error
        try:
            yield
        finally:
            fcntl.flock(stream, fcntl.LOCK_UN)


def run(command, cwd, logfile, env):
    command = [str(arg) for arg in command]
    log(f"{shlex.join(command)}\n  cwd: {cwd}\n  log: {logfile}")
    if exists(logfile):
        regular_file(logfile)
    with logfile.open("w") as stream:
        try:
            subprocess.run(command, cwd=cwd, env=env, stdout=stream,
                           stderr=subprocess.STDOUT, check=True)
        except subprocess.CalledProcessError as error:
            stream.flush()
            tail = "\n".join(logfile.read_text(errors="replace").splitlines()[-25:])
            raise BuildError(f"Command failed (exit {error.returncode}); see {logfile}\n{tail}") from error


def load_sources(path):
    sources = json.loads(path.read_text())
    for name in ("openjdk", "libffi"):
        source = sources[name]
        if not re.fullmatch(r"[0-9a-f]{64}", source["sha256"]):
            raise BuildError(f"Invalid SHA-256 for {name}")
        if not source["url"].startswith("https://"):
            raise BuildError(f"Expected an HTTPS source URL for {name}")
        if not re.fullmatch(r"[A-Za-z0-9_.+-]+", source["directory"]) or source["directory"] in (".", ".."):
            raise BuildError(f"Invalid archive root for {name}")
    if not re.fullmatch(r"[0-9a-f]{40}", sources["openjdk"]["commit"]):
        raise BuildError("Invalid OpenJDK commit")
    if not isinstance(sources["openjdk"].get("ported", True), bool):
        raise BuildError("OpenJDK ported must be a boolean (defaults to true)")
    return sources


def verify_archive(path, expected):
    actual = sha256(regular_file(path))
    if actual != expected:
        raise BuildError(f"SHA-256 mismatch: {path}\n  expected {expected}\n  actual   {actual}")


def download(source, cache):
    path = cache / (source["sha256"] + ".tar.gz")
    if exists(path):
        verify_archive(path, source["sha256"])
        log(f"Verified cached archive: {path}")
        return path
    log(f"Downloading {source['url']}")
    # Publish only a complete, verified download. Fail closed on corrupt cache
    # entries instead of silently deleting or replacing them.
    with tempfile.TemporaryDirectory(prefix=".download-", dir=cache) as temp:
        partial = Path(temp) / "archive.tar.gz"
        request = urllib.request.Request(source["url"], headers={"User-Agent": "scarlet-openjdk-bootstrap"})
        with urllib.request.urlopen(request, timeout=60) as response, partial.open("wb") as stream:
            shutil.copyfileobj(response, stream)
        verify_archive(partial, source["sha256"])
        partial.rename(path)
    return path


def extract_archive(archive, destination, root):
    """Extract only regular files/directories into a new tree; never follow links.

    Both pinned archives contain no links. Rejecting *all* links also prevents
    hard-link aliases and symlink chains from escaping or overwriting a tree.
    Validate the entire member list before writing any archive content.
    """
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        entries = {}
        for member in members:
            path = PurePosixPath(member.name)
            if (path.is_absolute() or ".." in path.parts or "\\" in member.name
                    or not path.parts or path.parts[0] != root
                    or not (member.isfile() or member.isdir())):
                raise BuildError(f"Unsafe tar member: {member.name!r}")
            if path in entries:
                raise BuildError(f"Duplicate tar member: {member.name!r}")
            if path == PurePosixPath(root) and not member.isdir():
                raise BuildError("Archive root must be a directory")
            entries[path] = member
        if not entries:
            raise BuildError(f"Empty source archive: {archive}")
        for path in entries:
            if any(parent in entries and not entries[parent].isdir() for parent in path.parents):
                raise BuildError(f"Tar member has a non-directory parent: {path}")
        destination.mkdir()  # Refuse an existing destination, including symlinks.
        for path, member in entries.items():
            relative = path.relative_to(root)
            target = destination / relative
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as src, target.open("xb") as dst:
                    shutil.copyfileobj(src, dst)
                target.chmod(0o755 if member.mode & 0o111 else 0o644)
                os.utime(target, (member.mtime, member.mtime))


def snapshot_overlay(overlay):
    if overlay.is_symlink() or not overlay.is_dir():
        raise BuildError(f"Missing regular overlay directory: {overlay}")
    files = {}
    for path in sorted(overlay.rglob("*")):
        if path.is_symlink():
            raise BuildError(f"Overlay symlinks are unsupported: {path}")
        if not path.is_dir():
            regular_file(path)
            files[path.relative_to(overlay).as_posix()] = (
                path.read_bytes(), bool(path.stat().st_mode & 0o111))
    return files


def snapshot_patches(patches_dir):
    if patches_dir.is_symlink() or not patches_dir.is_dir():
        raise BuildError(f"Missing regular patch directory: {patches_dir}")
    patches = {}
    for path in sorted(patches_dir.glob("*.patch")):
        data = regular_file(path).read_bytes()
        if not data.strip():
            raise BuildError(f"Empty patch: {path}")
        patches[path.name] = data
    if not patches:
        raise BuildError(f"No patches found in {patches_dir}")
    return patches


def file_hashes(files):
    return {name: hashlib.sha256(data).hexdigest() for name, data in sorted(files.items())}


def tree_identity(tree):
    regulars = []
    for path in sorted(tree.rglob("*")):
        if path.is_symlink():
            raise BuildError(f"Unexpected symlink in prepared sources: {path}")
        name = path.relative_to(tree).as_posix()
        if path.is_dir():
            regulars.append((name, "directory"))
        else:
            regular_file(path)
            regulars.append((name, sha256(path), stat.S_IMODE(path.stat().st_mode)))
    return identity(regulars)


def prepare_source(name, source, archive, sources_dir, env, patches=None, overlay=None):
    patches = patches or {}
    overlay = overlay or {}
    # A separator is required even when a patch lacks a final newline. Keep the
    # original per-file hashes as well as the exact bytes passed to patch.
    patch = b"\n".join(data if data.endswith(b"\n") else data + b"\n"
                       for _, data in sorted(patches.items()))
    inputs = {"source": source, "patches": file_hashes(patches),
              "patch_sha256": hashlib.sha256(patch).hexdigest() if patches else None,
              "overlay": {name: [hashlib.sha256(data).hexdigest(), executable]
                          for name, (data, executable) in sorted(overlay.items())}}
    destination = sources_dir / (name + "-" + identity(inputs))
    tree = destination / "tree"
    if exists(destination):
        directory(destination)
        receipt = json.loads(regular_file(destination / "source.json").read_text())
        if receipt["inputs"] != inputs or tree.is_symlink() or not tree.is_dir():
            raise BuildError(f"Unrecognized prepared source: {destination}")
        if receipt["tree_sha256"] != tree_identity(tree):
            raise BuildError(f"Prepared sources were modified: {tree}; preserve them and choose a new --output")
        log(f"Reusing verified sources: {tree}")
        return tree
    verify_archive(archive, source["sha256"])
    with tempfile.TemporaryDirectory(prefix=".source-", dir=sources_dir) as temp:
        stage = Path(temp) / "prepared"
        stage.mkdir()
        staged_tree = stage / "tree"
        log(f"Preparing {name}: {destination}")
        extract_archive(archive, staged_tree, source["directory"])
        if patches:
            patch_file = stage / "scarlet.patch"
            patch_file.write_bytes(patch)
            # This is a fresh extraction, so a failed/reversed patch is an error,
            # not a reason to guess whether an arbitrary user tree is patched.
            patch_log = sources_dir / (destination.name + "-patch.log")
            run(["patch", "--batch", "--forward", "--fuzz=0", "-p1", "-i", patch_file],
                staged_tree, patch_log, env)
        for name, (data, executable) in sorted(overlay.items()):
            path = staged_tree / name
            path.parent.mkdir(parents=True, exist_ok=True)
            if exists(path):
                regular_file(path)
            path.write_bytes(data)
            path.chmod(0o755 if executable else 0o644)
        write_json(stage / "source.json", {"inputs": inputs, "tree_sha256": tree_identity(staged_tree)})
        stage.rename(destination)
    return tree


def native_environment():
    env = os.environ.copy()
    missing = [name for name in NATIVE_VARS if not env.get(name)]
    if missing:
        raise BuildError("Enter the repository's Nix dev shell; missing " + ", ".join(missing))
    runtime = Path(env["SCARLET_NATIVE_RUNTIME"]).resolve()
    for name in ("include", "include/c++/v1", "lib"):
        if not (runtime / name).is_dir():
            raise BuildError(f"Incomplete SCARLET_NATIVE_RUNTIME: missing {runtime / name}")
    for name in ("libc++.a", "libc++abi.a", "libscarlet_c_startup.a", "libscarlet_cxx_support.a", "libscarlet_c.a"):
        if not (runtime / "lib" / name).is_file():
            raise BuildError(f"Incomplete native runtime: missing {name}")
    env["SCARLET_NATIVE_RUNTIME"] = str(runtime)
    for name in (*NATIVE_VARS[1:], "SCARLET_RUSTC"):
        value = env.get(name, "rustc")
        tool = shutil.which(value)
        if not tool:
            raise BuildError(f"{name} must name an executable, not a shell command: {value}")
        # Preserve clang++'s name even if it is a symlink in the SDK.
        env[name] = os.path.abspath(tool)
    sysroot = subprocess.check_output([env["SCARLET_RUSTC"], "--print", "sysroot"], env=env, text=True).strip()
    crt = Path(sysroot) / "lib/rustlib/aarch64-unknown-scarlet/lib/scarlet-crt0.o"
    if not crt.is_file():
        raise BuildError(f"Scarlet Rust CRT not found: {crt}")
    env["LC_ALL"] = "C"
    return env, {"environment": {name: env[name] for name in (*NATIVE_VARS, "SCARLET_RUSTC")},
                 "build_environment": {name: env[name] for name in BUILD_ENV_VARS if name in env},
                 "crt": str(crt), "crt_sha256": sha256(crt)}


def validate_boot_jdk(path, env, major=21):
    path = path.expanduser().resolve()
    for name in ("java", "javac", "jar", "jmod", "jlink"):
        if not os.access(path / "bin" / name, os.X_OK):
            raise BuildError(f"--boot-jdk must be a Java {major} JDK home; missing {path / 'bin' / name}")
    result = subprocess.run([str(path / "bin/java"), "-XshowSettings:properties", "-version"],
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=True)
    if not re.search(rf"^\s*java\.specification\.version\s*=\s*{major}\s*$", result.stdout, re.MULTILINE):
        raise BuildError(f"--boot-jdk must be Java {major}: {path}\n{result.stdout}")
    # Nix JDK homes may link release metadata into their immutable store.
    release = regular_file((path / "release").resolve())
    return path, {"home": str(path), "release_sha256": sha256(release)}


def source_version(source):
    """Read the upstream release numbers used by configure and image packaging."""
    data = regular_file(source / "make/conf/version-numbers.conf").read_text()
    parts = []
    for name in ("FEATURE", "INTERIM", "UPDATE", "PATCH", "EXTRA1", "EXTRA2", "EXTRA3"):
        match = re.search(rf"^DEFAULT_VERSION_{name}=([0-9]+)\s*$", data, re.MULTILINE)
        if not match:
            raise BuildError(f"Missing upstream DEFAULT_VERSION_{name}")
        parts.append(int(match[1]))
    while len(parts) > 1 and parts[-1] == 0:
        parts.pop()
    return parts[0], ".".join(map(str, parts))


def compiler_drivers(work, data):
    bindir = directory(work / "bin")
    for name in ("clang", "clang++"):
        # Configure resolves symlinks. cc.py selects C++ from argv[0], so these
        # MUST be independent regular executable files with their exact names.
        write_once(bindir / name, data, executable=True)
    return bindir / "clang", bindir / "clang++"


def build_libffi(source, work, cc, env, jobs):
    build = directory(work / "libffi")
    prefix = directory(work / "ffi")
    stamp = work / "libffi-configured.json"
    if not stamp.exists() or not (build / "Makefile").is_file():
        run(["bash", source / "configure", "--host=aarch64-unknown-none", "--disable-shared",
             "--disable-docs", "--disable-multi-os-directory", f"--prefix={prefix}",
             f"CC={cc}", f"AR={env['SCARLET_NATIVE_AR']}", f"RANLIB={env['SCARLET_NATIVE_RANLIB']}",
             f"CFLAGS={FFI_CFLAGS}"], build, work / "logs/libffi-configure.log", env)
        write_json(stamp, {"configured": True})
    run(["make", f"-j{jobs}"], build, work / "logs/libffi-build.log", env)
    run(["make", "install"], build, work / "logs/libffi-install.log", env)
    for name in ("include/ffi.h", "include/ffitarget.h", "lib/libffi.a"):
        regular_file(prefix / name)
    return prefix


def configure_hotspot(source, build, boot_jdk, ffi, cc, cxx, env, logfile, variant="zero", major=21):
    vm = "zero" if variant == "zero" else "server"
    features = JVM_FEATURES if variant == "zero" else "compiler1,-compiler2," + JVM_FEATURES
    command = ["bash", source / "configure", f"--with-conf-name=scarlet-{variant}",
               "--openjdk-target=aarch64-unknown-scarlet", "--with-toolchain-type=clang",
               f"--with-boot-jdk={boot_jdk}", f"--with-build-jdk={boot_jdk}",
               "--with-freetype=bundled", f"--with-libffi={ffi}", f"--with-jvm-variants={vm}",
               "--enable-headless-only", "--disable-warnings-as-errors",
               "--disable-precompiled-headers", "--with-debug-level=release",
               "--with-build-user=scarlet",
               "--with-native-debug-symbols=none", f"CC={cc}", f"CXX={cxx}",
               f"AR={env['SCARLET_NATIVE_AR']}", f"NM={env['SCARLET_NATIVE_NM']}",
               f"RANLIB={env['SCARLET_NATIVE_RANLIB']}", f"--with-jvm-features={features}"]
    if major == 21:
        command.append("--enable-static-build")
    run(command, build, logfile, env)


def audit_probe(probe, logfile, env, auditor=None):
    run([sys.executable, auditor or ROOT / "tools/elf_audit.py", "--machine", "aarch64", "--scarlet", probe],
        probe.parent, logfile, env)
    report, = json.loads(logfile.read_text())
    if (report.get("machine") != "aarch64" or report.get("osabi") != 0x53
            or report.get("elf_type") != "EXEC" or not report.get("entry")
            or report.get("interpreter") or report.get("needed") or report.get("rpath")
            or report.get("runpath") or report.get("undefined_relocated_symbols")
            or report.get("relocations") or report.get("errors") or report.get("error")):
        raise BuildError(f"Expected a static Scarlet AArch64 executable; see {logfile}")
    return report


def compile_runtime_support(work, cc, env, sources):
    # Compile the exact snapshots used in the build identity, even if another
    # task updates the repository's support files while this build is running.
    for name in ("runtime-support.c", "runtime-bridge.rs"):
        write_once(work / name, sources[name])
    support = work / "runtime-support.o"
    bridge = work / "libscarlet_jvm_bridge.a"
    for path in (support, bridge):
        if exists(path):
            regular_file(path)
    run([cc, "-O2", "-ffunction-sections", "-fdata-sections", "-c", work / "runtime-support.c",
         "-o", support], work, work / "logs/runtime-support-c.log", env)
    regular_file(support)
    run([env["SCARLET_RUSTC"], "--crate-type", "staticlib", "--target", "aarch64-unknown-scarlet",
         "-C", "panic=abort", "-O", work / "runtime-bridge.rs", "-o", bridge],
        work, work / "logs/runtime-support-rust.log", env)
    regular_file(bridge)
    return support, bridge


def build_runtime_support_probe(work, cc, cxx, env, sources):
    write_once(work / "runtime-support.cpp", sources["tests/runtime-support.cpp"])
    support, bridge = compile_runtime_support(work, cc, env, sources)
    probe = work / "runtime-support-probe"
    if exists(probe):
        regular_file(probe)
    run([cxx, "-O2", "-ffunction-sections", "-fdata-sections", work / "runtime-support.cpp",
         support, bridge, "-o", probe], work, work / "logs/runtime-support-link.log", env)
    return regular_file(probe)


def stage_java_helpers(work, sources, overlay, driver_data, cc_data, audit_data):
    """Run hashed helper snapshots; fork builds need no local OpenJDK sources."""
    root = directory(work / "inputs")
    tools = directory(root / "tools")
    helpers = directory(tools / "openjdk")
    for name, data in {**sources, "build.py": driver_data, "cc.py": cc_data}.items():
        write_once(helpers / name, data)
    write_once(tools / "elf_audit.py", audit_data)
    for name, (data, executable) in sorted((overlay or {}).items()):
        parent = helpers
        for part in ("overlay", *PurePosixPath(name).parts[:-1]):
            parent = directory(parent / part)
        write_once(parent / PurePosixPath(name).name, data, executable=executable)
    return helpers


def build_java(source, jdk_build, boot_jdk, ffi, jvm, work, cc, cxx, env, jobs, sources, helpers):
    run(["make", "java.base-java", f"JOBS={jobs}"], jdk_build, work / "logs/java-base-build.log", env)
    regular_file(jdk_build / "support/headers/java.base/java_lang_System.h")
    regular_file(jdk_build / "jdk/modules/java.base/module-info.class")
    core = work / "core-libs"
    run([sys.executable, "-B", helpers / "core-libs.py", "--source", source, "--build", jdk_build,
         "--ffi", ffi / "lib/libffi.a", "--output", core, "--jobs", min(jobs, 16)],
        work, work / "logs/core-libs-build.log", env)
    archives = [jvm, *(regular_file(core / name) for name in CORE_ARCHIVES)]
    builtins = work / "builtins.c"
    if exists(builtins):
        regular_file(builtins)
    run([sys.executable, "-B", helpers / "builtin-symbols.py", "--nm", env["SCARLET_NATIVE_NM"],
         "--output", builtins, *archives], work, work / "logs/builtin-symbols.log", env)
    regular_file(builtins)
    write_once(work / "launcher.c", sources["launcher.c"])
    launcher_obj, builtins_obj = work / "launcher.o", work / "builtins.o"
    java = work / "java"
    for path in (launcher_obj, builtins_obj, java):
        if exists(path):
            regular_file(path)
    run([cc, "-O2", "-I", source / "src/java.base/share/native/include",
         "-I", source / "src/java.base/scarlet/native/include", "-c", work / "launcher.c",
         "-o", launcher_obj], work, work / "logs/launcher-build.log", env)
    regular_file(launcher_obj)
    run([cc, "-O2", "-c", builtins, "-o", builtins_obj], work, work / "logs/builtins-build.log", env)
    regular_file(builtins_obj)
    support, bridge = compile_runtime_support(work, cc, env, sources)
    ffi_archive = regular_file(ffi / "lib/libffi.a")
    run([cxx, support, bridge, launcher_obj, builtins_obj, *archives, ffi_archive,
         "-Wl,--error-limit=0", "-o", java], work, work / "logs/java-link.log", env)
    regular_file(java)
    elf = audit_probe(java, work / "logs/java-elf.json", env, helpers.parent / "elf_audit.py")
    image_root = work / "image"
    run([sys.executable, "-B", helpers / "image.py", "--source", source, "--build", jdk_build,
         "--boot-jdk", boot_jdk, "--java", java, "--output", image_root],
        work, work / "logs/java-image.log", env)
    image_receipt = json.loads(regular_file(image_root / "image.json").read_text())
    artifact = regular_file(image_root / "bin/java")
    java_home = Path(image_receipt["java_home"])
    regular_file(java_home / "lib/modules")
    if sha256(artifact) != elf["sha256"] or image_receipt["java_sha256"] != elf["sha256"]:
        raise BuildError("Packaged java does not match the audited native launcher")
    return {"artifact": str(artifact), "artifact_sha256": elf["sha256"],
            "java_home": str(java_home), "image_root": str(image_root), "image": image_receipt,
            "elf": dict(elf, path=str(artifact)),
            "archives": {str(path): sha256(path) for path in [*archives, ffi_archive]},
            "scope": "Native Java and Scarlet java.base image compiled, packaged and ELF audited; not guest-tested"}


def build(args):
    env, toolchain = native_environment()
    source_names = {"hotspot": ("libffi", "openjdk"), "java": ("libffi", "openjdk"), "runtime-probe": ("libffi",),
                    "runtime-support-probe": ()}[args.target]
    sources = load_sources(HERE / "sources.json") if source_names else {}
    cc_data = regular_file(HERE / "cc.py").read_bytes()
    driver_data = regular_file(Path(__file__)).read_bytes()
    boot_jdk, boot_identity, overlay = None, None, None
    patches = {}
    if args.target in ("hotspot", "java"):
        version = re.fullmatch(r"jdk-(21|25)\.[0-9.]+\+[0-9]+", sources["openjdk"]["version"])
        if not version:
            raise BuildError("Expected a supported JDK 21 or 25 release in sources.json")
        major = int(version[1])
        boot_jdk, boot_identity = validate_boot_jdk(args.boot_jdk, env, major)
        if not sources["openjdk"].get("ported", True):
            patches = snapshot_patches(HERE / "patches")
            overlay = snapshot_overlay(HERE / "overlay")
    probe_names = {"hotspot": (), "java": JAVA_INPUTS, "runtime-probe": ("tests/runtime.c",),
                   "runtime-support-probe": ("tests/runtime-support.cpp", "runtime-support.c", "runtime-bridge.rs")}
    probe_sources = {name: regular_file(HERE / name).read_bytes() for name in probe_names[args.target]}
    audit_data = regular_file(ROOT / "tools/elf_audit.py").read_bytes() if probe_sources else None
    output = args.output.expanduser().absolute()
    with output_lock(output):
        output = output.resolve()
        if source_names:
            cache = directory(output / "downloads")
            source_dir = directory(output / "sources")
        trees = {}
        for name in source_names:
            archive = download(sources[name], cache)
            trees[name] = prepare_source(name, sources[name], archive, source_dir, env,
                                         patches if name == "openjdk" else None,
                                         overlay if name == "openjdk" else None)
        inputs = {"driver_sha256": hashlib.sha256(driver_data).hexdigest(), "compiler_sha256": hashlib.sha256(cc_data).hexdigest(),
                  "sources": {name: str(tree) for name, tree in trees.items()}, "toolchain": toolchain,
                  "source_archives": {name: sources[name] for name in source_names},
                  "boot_jdk": boot_identity, "target": args.target, "patches": file_hashes(patches),
                  "jvm_variant": args.jvm_variant if args.target in ("hotspot", "java") else None,
                  "local_sources": file_hashes(probe_sources),
                  "overlay": {name: [hashlib.sha256(data).hexdigest(), executable]
                              for name, (data, executable) in sorted((overlay or {}).items())},
                  "audit_sha256": hashlib.sha256(audit_data).hexdigest() if audit_data is not None else None}
        builds = directory(output / "builds")
        work = builds / identity(inputs)
        if exists(work):
            directory(work)
            if json.loads(regular_file(work / "inputs.json").read_text()) != inputs:
                raise BuildError(f"Refusing unrecognized build directory: {work}")
        else:
            work.mkdir()
            write_once(work / "inputs.json", json_bytes(inputs))
        directory(work / "logs")
        cc, cxx = compiler_drivers(work, cc_data)
        if args.target == "java":
            helpers = stage_java_helpers(work, probe_sources, overlay, driver_data, cc_data, audit_data)
        receipt = {"inputs": inputs, "work": str(work)}
        if "libffi" in trees:
            ffi = build_libffi(trees["libffi"], work, cc, env, args.jobs)
            receipt["libffi"] = str(ffi)
        if args.target == "runtime-probe":
            probe_source, probe = work / "runtime.c", work / "runtime-probe"
            write_once(probe_source, probe_sources["tests/runtime.c"])
            if exists(probe):
                regular_file(probe)
            run([cc, "-O2", "-ffunction-sections", "-fdata-sections", "-I", ffi / "include",
                 probe_source, ffi / "lib/libffi.a", "-o", probe], work, work / "logs/runtime-build.log", env)
            receipt["elf"] = audit_probe(probe, work / "logs/runtime-elf.json", env)
            receipt.update(artifact=str(probe), scope="Native prerequisites compiled and ELF audited; not executed")
        elif args.target == "runtime-support-probe":
            probe = build_runtime_support_probe(work, cc, cxx, env, probe_sources)
            receipt["elf"] = audit_probe(probe, work / "logs/runtime-support-elf.json", env)
            receipt.update(artifact=str(probe),
                           scope="Environment and C++ TLS destructor probe compiled and ELF audited; not executed")
        else:
            jdk_build = directory(work / "hotspot")
            stamp = work / "hotspot-configured.json"
            if not stamp.exists() or not (jdk_build / "spec.gmk").is_file():
                configure_hotspot(trees["openjdk"], jdk_build, boot_jdk, ffi, cc, cxx, env,
                                  work / "logs/hotspot-configure.log", args.jvm_variant, major)
                write_json(stamp, {"configured": True})
            if args.configure_only:
                receipt.update(scope="HotSpot configured only; not compiled", build_directory=str(jdk_build))
            else:
                target = "hotspot-static-libs" if major >= 25 else "hotspot"
                run(["make", target, f"JOBS={args.jobs}"], jdk_build, work / "logs/hotspot-build.log", env)
                vm = "zero" if args.jvm_variant == "zero" else "server"
                archive = (f"hotspot/variant-{vm}/libjvm/objs/static/libjvm.a" if major >= 25
                           else f"support/modules_libs/java.base/{vm}/libjvm.a")
                artifact = regular_file(jdk_build / archive)
                if args.target == "java":
                    receipt.update(build_java(trees["openjdk"], jdk_build, boot_jdk, ffi, artifact,
                                              work, cc, cxx, env, args.jobs, probe_sources, helpers))
                else:
                    receipt.update(artifact=str(artifact), artifact_sha256=sha256(artifact),
                                   scope="Static HotSpot archive only; not a runnable JDK or JVM execution test")
        write_json(work / "result.json", receipt)
        write_json(output / (args.target + ".json"), receipt)
        log(receipt["scope"])
        log("Result: " + receipt.get("artifact", receipt.get("build_directory", str(work))))
        if "java_home" in receipt:
            log(f"Java home: {receipt['java_home']}\n  Image root: {receipt['image_root']}")
        log(f"Receipt: {output / (args.target + '.json')}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--boot-jdk", type=Path, help="Matching host JDK 21 or 25 home (required for hotspot/java; also used as build JDK)")
    parser.add_argument("--output", type=Path, default=ROOT / ".build/openjdk", help="Driver-owned output (default: %(default)s)")
    parser.add_argument("--target", choices=("hotspot", "java", "runtime-probe", "runtime-support-probe"), default="hotspot")
    parser.add_argument("--jvm-variant", choices=("c1", "zero"), default="c1",
                        help="C1 AArch64 JIT (default) or portable Zero interpreter")
    parser.add_argument("--jobs", type=int, default=min(8, os.cpu_count() or 1))
    parser.add_argument("--configure-only", action="store_true", help="Build libffi and configure hotspot, without compiling hotspot")
    args = parser.parse_args(argv)
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    if args.target in ("hotspot", "java") and args.boot_jdk is None:
        parser.error("--boot-jdk is required for hotspot/java (matching JDK major)")
    if args.configure_only and args.target != "hotspot":
        parser.error("--configure-only requires --target hotspot")
    try:
        build(args)
    except (BuildError, OSError, ValueError, KeyError, tarfile.TarError, subprocess.CalledProcessError) as error:
        print(f"[openjdk] ERROR: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("[openjdk] Interrupted; no source trees were reset", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())
