#!/usr/bin/env python3
"""Run Bionic, ART, or diagnose a program directly on Scarlet's Android ABI.

No Android/Linux guest kernel is involved. An optional runtime root must contain
real Android runtime files (NDK .so files are link stubs, not runtime libraries).
"""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

from evidence import analyze_art, analyze_direct, analyze_libbinder, analyze_selinux, analyze_services, analyze_framework, analyze_framework_services
from art_inputs import art_launch, validate_dex, verify_art_runtime, sha256
from libbinder_inputs import verify_libbinder_build
from selinux_inputs import verify_selinux_build
from service_inputs import verify_service_inputs, stage_service_inputs, CONTEXTS
from stock_art import stage_stock_inputs, verify_stock_inputs
from framework_inputs import verify_framework_inputs, verify_framework_probe, framework_launch, stage_framework_inputs, PROGRAM

sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parent
ROOT = SOURCE.parents[1]
STOCK_CONFIG = SOURCE / "checkpoints/2026-10-28-cloud/stock-art/generated-art-ld.config.txt"
spec = importlib.util.spec_from_file_location("android_smoke", ROOT / "projects/aarch64-limine-crosvm/tools/fixture.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--probes", type=Path)
    mode.add_argument("--art-dex", type=Path, help="run only dalvikvm64 -Xint with this classes.dex")
    mode.add_argument("--program", type=Path, help="diagnose this Android executable (exit is not a service-readiness test)")
    mode.add_argument("--libbinder-probe", type=Path, help="build directory from build-libbinder-probe.py; verify stock-library RPC and lifetime")
    mode.add_argument("--selinux-probe", type=Path, help="build directory from build-selinux-probe.py; verify Shield allow/deny through stock libselinux")
    mode.add_argument("--service-probe", type=Path, help="stock servicemanager integration probe build")
    mode.add_argument("--framework-probe", type=Path, help="build-dex.py --framework-sdk output; verify stock app_process framework/JNI")
    mode.add_argument("--framework-services", type=Path, help="framework probe build; stock manager and three labeled Java peers")
    parser.add_argument("--framework-inputs", type=Path, help="prepare-framework.py output")
    parser.add_argument("--service-inputs", type=Path, help="verified stock servicemanager from prepare-servicemanager.py")
    parser.add_argument("--runtime-root", type=Path, help="Android /system and /apex runtime tree")
    parser.add_argument("--linker-debug", action="store_true", help="enable Android LD_DEBUG=3 for ART diagnosis")
    parser.add_argument("--stock-inputs", type=Path, help="verified stock property/APEX overlay for ART or a program")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--accel", choices=("tcg", "hvf"), default="tcg")
    parser.add_argument("--binder", action="store_true", help="include Binder RPC and three-process object lifetime checks")
    args = parser.parse_args()
    if args.framework_services:
        if not args.service_inputs:
            parser.error("--framework-services requires --service-inputs")
        args.framework_probe = args.framework_services
    framework = None
    if args.framework_probe:
        if not args.runtime_root or not args.stock_inputs or not args.framework_inputs:
            parser.error("--framework-probe requires --runtime-root, --stock-inputs and --framework-inputs")
        verify_framework_probe(args.framework_probe, services=bool(args.framework_services))
        framework = verify_framework_inputs(args.framework_inputs, args.runtime_root, args.stock_inputs)
        args.program = args.framework_inputs / PROGRAM
    elif args.framework_inputs:
        parser.error("--framework-inputs requires --framework-probe")
    if args.service_probe:
        if not args.runtime_root or not args.stock_inputs or not args.service_inputs:
            parser.error("--service-probe requires --runtime-root, --stock-inputs and --service-inputs")
        args.program = verify_libbinder_build(args.service_probe, args.runtime_root, service=True)
        verify_service_inputs(args.service_inputs, args.runtime_root)
    elif args.service_inputs:
        if not args.framework_services:
            parser.error("--service-inputs requires --service-probe or --framework-services")
        verify_service_inputs(args.service_inputs, args.runtime_root)
    if args.selinux_probe:
        if not args.runtime_root or not args.stock_inputs:
            parser.error("--selinux-probe requires --runtime-root and --stock-inputs")
        args.program = verify_selinux_build(args.selinux_probe, args.runtime_root)
    if args.libbinder_probe:
        if not args.runtime_root or not args.stock_inputs:
            parser.error("--libbinder-probe requires --runtime-root and --stock-inputs")
        args.program = verify_libbinder_build(args.libbinder_probe, args.runtime_root)
    if (args.art_dex or args.program) and (not args.runtime_root or args.binder):
        parser.error("--art-dex/--program requires --runtime-root and cannot use --binder")
    if args.linker_debug and not (args.art_dex or args.program):
        parser.error("--linker-debug requires --art-dex or --program")
    if args.stock_inputs and not (args.art_dex or args.program):
        parser.error("--stock-inputs requires --art-dex or --program")
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("timeout must be finite and positive")
    kernel, output = args.kernel.resolve(), args.output.resolve()
    probes = args.probes.resolve() if args.probes else None
    if output.exists():
        parser.error("use a new output directory to preserve earlier evidence")
    inputs = [kernel]
    if args.program:
        inputs.append(args.program)
    if probes:
        inputs += [probes / n for n in ["bionic-static", "bionic-dynamic", "libbionic-probe.so", "build.json"]]
    for path in inputs:
        if not path.is_file():
            parser.error(f"missing input: {path}")
    if args.runtime_root and not args.runtime_root.is_dir():
        parser.error("runtime root must be a directory")
    runtime_files = []
    art = None
    dex_hash = None
    if args.runtime_root:
        runtime_files = [args.runtime_root / name for name in (
            "system/bin/linker64", "apex/com.android.runtime/lib64/bionic/libc.so",
            "apex/com.android.runtime/lib64/bionic/libdl.so", "apex/com.android.runtime/lib64/bionic/libm.so")]
        for path in runtime_files:
            if not path.is_file():
                parser.error(f"missing Android runtime file: {path}")
    if args.art_dex or args.program:
        if args.art_dex:
            dex_hash = validate_dex(args.art_dex)
        metadata = verify_art_runtime(args.runtime_root)
        art = art_launch(metadata) if args.art_dex else {
            "argv": ["/system/bin/scarlet-program-probe"],
            "env": ["ANDROID_LOG_TAGS=*:v", "LD_LIBRARY_PATH=" + metadata["library_path"]],
        }
        if framework:
            art = framework_launch(framework, services=bool(args.framework_services))
            dex_hash = validate_dex(args.framework_probe / "classes.dex")
        if args.linker_debug:
            art["env"].append("LD_DEBUG=3")
        if args.stock_inputs:
            verify_stock_inputs(args.stock_inputs, args.runtime_root)
            art["env"] = [value for value in art["env"] if not value.startswith("LD_LIBRARY_PATH=")]
        runtime_files = [args.runtime_root / name for name in metadata["files"]]
    code = smoke.firmware(("SCARLET_EFI_CODE_ARM64_HVF", "SCARLET_EFI_CODE_ARM64")) if args.accel == "hvf" else smoke.firmware(("SCARLET_EFI_CODE_ARM64", "SCARLET_EFI_CODE_ARM64_EL2"))
    variables = smoke.firmware(("SCARLET_EFI_VARS_ARM64_HVF", "SCARLET_EFI_VARS_ARM64")) if args.accel == "hvf" else smoke.firmware(("SCARLET_EFI_VARS_ARM64", "SCARLET_EFI_VARS_ARM64_EL2"))
    output.mkdir(parents=True)
    # The input build may be replaced by another cargo invocation. Bind both
    # image construction and evidence hashes to one immutable run-local copy.
    shutil.copy2(kernel, output / "scarlet.elf")
    kernel = output / "scarlet.elf"
    (output / "inputs.json").write_text(json.dumps({
        "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
        "dex_sha256": dex_hash,
        "framework_manifest_sha256": sha256(args.framework_inputs / "framework-inputs.json") if framework else None,
        "framework_build_sha256": sha256(args.framework_probe / "build.json") if framework else None,
        "framework_fixture_sha256": {name: sha256(SOURCE / "init/src" / name) for name in (("main.rs", "stock.rs", "services.rs", "framework_services.rs") if args.framework_services else ("main.rs", "stock.rs"))} if framework else None,
        "program_sha256": sha256(args.program) if args.program else None,
        "libbinder_build_sha256": sha256(args.libbinder_probe / "build.json") if args.libbinder_probe else None,
        "service_build_sha256": sha256(args.service_probe / "build.json") if args.service_probe else None,
        "service_manifest_sha256": sha256(args.service_inputs / "service-inputs.json") if args.service_inputs else None,
        "service_fixture_sha256": sha256(SOURCE / "init/src/services.rs") if args.service_probe else None,
        "service_contexts_sha256": hashlib.sha256(CONTEXTS.encode()).hexdigest() if args.service_inputs else None,
        "selinux_build_sha256": sha256(args.selinux_probe / "build.json") if args.selinux_probe else None,
        "shield_fixture_sha256": sha256(SOURCE / "init/src/shield.rs") if args.selinux_probe else None,
        "stock_reference_config_sha256": sha256(STOCK_CONFIG) if args.stock_inputs else None,
        "stock_manifest_sha256": sha256(args.stock_inputs / "stock-inputs.json") if args.stock_inputs else None,
        "runtime_sha256": {str(p.relative_to(args.runtime_root)): hashlib.sha256(p.read_bytes()).hexdigest()
                           for p in runtime_files},
    }, indent=2) + "\n")
    if framework:
        shutil.copy2(args.framework_probe / "build.json", output / "framework-build.json")
        shutil.copy2(args.framework_inputs / "framework-inputs.json", output / "framework-inputs.json")
    if args.service_probe:
        shutil.copy2(args.service_probe / "build.json", output / "service-build.json")
    if args.service_inputs:
        shutil.copy2(args.service_inputs / "service-inputs.json", output / "service-inputs.json")
    if args.libbinder_probe:
        shutil.copy2(args.libbinder_probe / "build.json", output / "libbinder-build.json")
    if args.selinux_probe:
        shutil.copy2(args.selinux_probe / "build.json", output / "selinux-build.json")
    (output / "result.json").write_text('{"result":"PREPARING"}\n')
    staging = output / "staging"
    android = staging / "systems/android-aarch64"
    boot = output / "boot"
    for tree in (staging, boot):
        for name in ["bin", "dev", "mnt/newroot", "home", "shared", "tmp", "root"]:
            (tree / name).mkdir(parents=True, exist_ok=True)
    for name in ["bin", "lib", "system/bin", "dev", "proc", "sys", "tmp", "root"]:
        (android / name).mkdir(parents=True, exist_ok=True)
    if args.selinux_probe or args.service_probe or args.framework_services:
        (android / "sys/fs/selinux").mkdir(parents=True, exist_ok=True)
    if args.runtime_root:
        shutil.copytree(args.runtime_root, android, dirs_exist_ok=True, symlinks=True)
    if art:
        if args.art_dex or framework:
            shutil.copy2(args.art_dex or args.framework_probe / "classes.dex", android / "data/local/tmp/classes.dex")
            if validate_dex(android / "data/local/tmp/classes.dex") != dex_hash:
                raise ValueError("DEX changed during staging")
        verify_art_runtime(android)
        shutil.copy2(args.runtime_root / "art-runtime.json", output / "art-runtime.json")
        if args.stock_inputs:
            stage_stock_inputs(args.stock_inputs, args.runtime_root, android)
            shutil.copy2(args.stock_inputs / "stock-inputs.json", output / "stock-inputs.json")
        if args.service_inputs:
            stage_service_inputs(args.service_inputs, args.runtime_root, android)
        if framework:
            bootstrap = stage_framework_inputs(args.framework_inputs, args.runtime_root, args.stock_inputs, android)
            (output / "framework-bootstrap.json").write_text(json.dumps(bootstrap, indent=2) + "\n")
        if args.program and not framework:
            target = android / art["argv"][0].lstrip("/")
            if target.exists() or target.is_symlink():
                raise ValueError("program path already exists in runtime")
            shutil.copy2(args.program, target)
            recorded = json.loads((output / "inputs.json").read_text())["program_sha256"]
            if sha256(target) != recorded:
                raise ValueError("program changed during staging")
    else:
        for name in ["bionic-static", "bionic-dynamic"]:
            shutil.copy2(probes / name, android / "bin" / name)
        control = staging / "systems/linux-aarch64/bin"
        control.mkdir(parents=True)
        shutil.copy2(probes / "bionic-static", control / "bionic-static")
        shutil.copy2(probes / "libbionic-probe.so", android / "lib/libbionic-probe.so")
        shutil.copy2(probes / "build.json", output / "probes.json")
    env = dict(os.environ, SCARLET_ANDROID_DYNAMIC="1" if args.runtime_root else "0",
               SCARLET_ANDROID_BINDER="1" if args.binder else "0",
               SCARLET_ANDROID_STOCK="1" if args.stock_inputs else "0",
               SCARLET_ANDROID_SERVICES="1" if args.service_probe else "0",
               SCARLET_ANDROID_SHIELD="1" if args.selinux_probe else "0",
               SCARLET_ANDROID_ART="1" if art else "0",
               SCARLET_ANDROID_PROGRAM="1" if args.program else "0",
               SCARLET_ANDROID_FRAMEWORK="1" if framework else "0",
               SCARLET_ANDROID_FRAMEWORK_SERVICES="1" if args.framework_services else "0",
               SCARLET_ART_ARGS="\x1f".join(art["argv"]) if art else "",
               SCARLET_ART_ENV="\x1f".join(art["env"]) if art else "")
    # Preserve the exact launch even if native compilation or image creation fails.
    if art:
        (output / ("program-launch.json" if args.program else "art-launch.json")).write_text(json.dumps(art, indent=2) + "\n")
    for manifest, binary, destination in [
        (SOURCE / "init/Cargo.toml", "android-direct-init", "init"),
        (ROOT / "user/scarlet-ld/Cargo.toml", "scarlet-ld", "bin/scarlet-ld"),
    ]:
        subprocess.run(["cargo", "build", "--manifest-path", str(manifest), "--release",
                        "--target", "aarch64-unknown-scarlet", "--target-dir", str(output / "target")], env=env, check=True)
        compiled = output / "target/aarch64-unknown-scarlet/release" / binary
        for tree in (staging, boot):
            shutil.copy2(compiled, tree / destination)
    rootfs = output / "rootfs.ext2"
    size = sum(p.stat().st_size for p in staging.rglob("*") if p.is_file() and not p.is_symlink())
    with rootfs.open("xb") as f:
        f.truncate(max(128 * 1024**2, size * 2 + 64 * 1024**2))
    with (output / "rootfs-build.log").open("wb") as log:
        subprocess.run(["mkfs.ext2", "-F", "-d", str(staging), str(rootfs)], stdout=log, stderr=subprocess.STDOUT, check=True)
    archive, image = output / "initramfs.cpio", output / "boot.img"
    smoke.create_initramfs(boot, archive)
    build = ["cargo-scarlet-plugin-limine", "--arch", "aarch64", "--kernel", str(kernel),
             "--initramfs", str(archive), "--output", str(image), "--cache-dir", str(output / "limine-cache"),
             "--cmdline", "console=ttyAMA0 init=/init"]
    with (output / "image-build.log").open("wb") as log:
        subprocess.run(build, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    runtime_vars = output / "efi-vars.fd"
    shutil.copyfile(variables, runtime_vars)
    runtime_vars.chmod(0o600)
    command = ["qemu-system-aarch64", "-machine", "virt,gic-version=3,acpi=off", "-cpu",
               "host" if args.accel == "hvf" else "max", "-m", "2G", "-smp", "1", "-accel", args.accel,
               "-display", "none", "-monitor", "none", "-serial", "stdio", "-no-reboot",
               "-drive", f"if=pflash,format=raw,unit=0,file={smoke.qemu_filename(code)},readonly=on",
               "-drive", f"if=pflash,format=raw,unit=1,file={smoke.qemu_filename(runtime_vars)}",
               "-global", "virtio-mmio.force-legacy=false",
               "-drive", f"id=boot,file={smoke.qemu_filename(image)},format=raw,if=none",
               "-device", "virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0",
               "-drive", f"id=root,file={smoke.qemu_filename(rootfs)},format=raw,if=none",
               "-device", "virtio-blk-device,drive=root,bus=virtio-mmio-bus.1",
               "-device", "virtio-rng-device,bus=virtio-mmio-bus.2", "-nic", "none"]
    (output / "commands.json").write_text(json.dumps({"image": build, "qemu": command}, indent=2) + "\n")
    prefix = b"SCARLET_ANDROID_PROGRAM" if args.program else (b"SCARLET_ANDROID_ART" if art else b"SCARLET_ANDROID_DIRECT")
    if args.service_probe:
        prefix = b"SCARLET_SERVICES"
    if args.framework_services:
        prefix = b"SCARLET_FRAMEWORK_SERVICES"
    smoke.SUCCESS = re.compile(rb"\n" + prefix + (rb"_DONE\r*\n" if args.program and not (args.service_probe or args.framework_services) else rb"_OK\r*\n"))
    smoke.FAILURE = re.compile(rb"\n" + prefix + rb"_FAIL:")
    result = smoke.run_guest(command, output, args.timeout)
    result["dynamic_runtime"] = bool(args.runtime_root)
    data = (output / "serial.log").read_bytes()
    if args.framework_services:
        result["framework_services"] = analyze_framework_services(data)
        if result["result"] == "PASS" and result["framework_services"]["result"] != "PASS":
            result["result"] = "missing or failed Java service evidence"
        result["apk_execution"] = "not tested"
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(f"Stock Java Binder service integration: {result['result']}")
        return 0 if result["result"] == "PASS" else 1
    if args.service_probe:
        result["services"] = analyze_services(data)
        if result["result"] == "PASS" and result["services"]["result"] != "PASS":
            result["result"] = "missing or failed service integration evidence"
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(f"Stock servicemanager integration: {result['result']}")
        return 0 if result["result"] == "PASS" else 1
    if args.program:
        # A daemon exiting (even with status zero), or merely surviving until
        # timeout, never proves that its service interface is usable.
        exits = re.findall(rb"\nSCARLET_ANDROID_PROGRAM_EXIT (-?\d+)\r*\n", data)
        result["program_exit_status"] = int(exits[0]) if len(exits) == 1 else None
        if framework:
            result["framework"] = analyze_framework(data)
            if result["result"] == "PASS" and result["framework"]["result"] != "PASS":
                result["result"] = "missing or failed framework evidence"
            result["apk_execution"] = "not tested"
            (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(f"Stock app_process framework/JNI probe: {result['result']}")
            return 0 if result["result"] == "PASS" else 1
        if args.selinux_probe:
            result["selinux"] = analyze_selinux(data)
            if result["result"] == "PASS" and result["selinux"]["result"] != "PASS":
                result["result"] = "missing or failed Shield evidence"
            result["servicemanager"] = "not tested"
            (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(f"Stock libselinux / Shield probe: {result['result']}")
            return 0 if result["result"] == "PASS" else 1
        if args.libbinder_probe:
            evidence = analyze_libbinder(data)
            result["libbinder"] = evidence
            if result["result"] == "PASS" and evidence["result"] != "PASS":
                result["result"] = "missing or failed libbinder evidence"
            result["service_readiness"] = ("custom BBinder context and transferred services verified"
                                           if result["result"] == "PASS" else "not established")
            result["servicemanager"] = "not tested"
            (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(f"Stock libbinder probe: {result['result']}")
            return 0 if result["result"] == "PASS" else 1
        result["service_readiness"] = "not tested"
        if result["result"] == "PASS":
            result["result"] = "EXITED" if len(exits) == 1 else "INCOMPLETE"
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(f"Android program diagnostic: {result['result']}; exit={result['program_exit_status']}")
        return 0 if result["result"] == "EXITED" and result["program_exit_status"] == 0 else 1
    evidence = analyze_art(data, stock=bool(args.stock_inputs)) if art else analyze_direct(data, dynamic=bool(args.runtime_root), binder=args.binder)
    result["art" if art else "probes"] = evidence
    if result["result"] == "PASS" and evidence["result"] != "PASS":
        result["result"] = "missing or failed child evidence"
    (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"{'ART' if art else 'Bionic'} direct execution: {result['result']}")
    return 0 if result["result"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
