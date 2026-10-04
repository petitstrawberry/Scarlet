#!/usr/bin/env python3
"""Build the pinned native SGFX driver and configure userspace linkage."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

sys.dont_write_bytecode = True
REPO = Path(__file__).resolve().parents[1]
SGFX_URL = "https://github.com/petitstrawberry/sgfx"
TARGETS = {"aarch64": "aarch64-unknown-scarlet", "riscv64": "riscv64gc-unknown-scarlet",
           "riscv32": "riscv32gc-unknown-scarlet"}


def run(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def project_target(project):
    manifest = tomllib.loads((project / "scarlet.toml").read_text())
    bsp = project / manifest["bsp"]["path"]
    config = tomllib.loads((bsp / ".cargo/config.toml").read_text())
    name = Path(config["build"]["target"]).name
    for arch, target in TARGETS.items():
        if name.startswith(arch):
            return arch, target
    raise RuntimeError(f"unsupported native SGFX target: {name}")


def source_checkout(revision, override):
    if override:
        source = override.resolve()
    else:
        source = REPO / ".scarlet/sgfx/sources" / revision
        if not source.exists():
            source.parent.mkdir(parents=True, exist_ok=True)
            run(["git", "clone", "--filter=blob:none", "--no-checkout", SGFX_URL, source])
        run(["git", "-C", source, "checkout", "--detach", revision])
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if actual != revision:
        raise RuntimeError(f"SGFX source revision {actual} differs from pinned {revision}")
    return source


def audit_driver(path, arch):
    spec = importlib.util.spec_from_file_location("scarlet_elf_audit", REPO / "tools/elf_audit.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    elf = module.Elf(path)
    report = elf.report()
    if (report["osabi"] != 83 or report["machine"] != arch or report["entry"] != 0
            or report["interpreter"] or report["needed"] or report["undefined_relocated_symbols"]
            or report["tls"] or report["relr"] or report["textrel"] or report["symbol_versioning"]
            or set(report["relocations"]) - {"NONE", "RELATIVE", "JUMP_SLOT", "GLOB_DAT", "ABS64", "64"}):
        raise RuntimeError(f"driver requires unsupported native linkage: {report}")
    _, count = elf.unpack("II", elf.at_vaddr(elf.tag(4), 8))
    exports = set()
    for index in range(1, count):
        name, info, _, section, _, _ = elf.unpack("IBBHQQ", elf.at_vaddr(elf.tag(6) + index * 24, 24))
        if section and info >> 4 in (1, 2):
            exports.add(elf.dynstring(name))
    if exports != {"sgfx_backend_get_api_v2", "sgfx_backend_get_driver_api_v2"}:
        raise RuntimeError(f"unexpected driver exports: {sorted(exports)}")
    report["exports"] = sorted(exports)
    return report


def audit_client_linkage(config, target):
    graph = subprocess.check_output(
        ["cargo", "--config", str(config), "tree", "--locked", "--workspace",
         "--manifest-path", str(REPO / ".cargo/Cargo.toml"), "--target", target,
         "--prefix", "none", "--edges", "normal,build"], text=True, cwd=REPO)
    if any(line.startswith("sgfx-backend-scarlet-virgl v") for line in graph.splitlines()):
        raise RuntimeError("native clients must load VirGL dynamically, not link its Rust backend")
    print("Native client dependency audit: VirGL is dynamic; no static VirGL implementation")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, default=Path.cwd())
    parser.add_argument("--sgfx-source", type=Path, help="use an existing checkout at the pinned revision")
    parser.add_argument("--install-dir", type=Path, help="also install the library and manifest into this directory")
    parser.add_argument("--check-client-linkage", action="store_true",
                        help="reject static VirGL in the native 64-bit userspace dependency graph")
    args = parser.parse_args()
    project = args.project.resolve()
    arch, target = project_target(project)
    dependency = tomllib.loads((REPO / "user/std-bin/Cargo.toml").read_text())["dependencies"]["sgfx"]
    source = source_checkout(dependency["rev"], args.sgfx_source)
    output = project / ".scarlet/sgfx"
    output.mkdir(parents=True, exist_ok=True)
    flags = ["--cfg", 'getrandom_backend="custom"']
    install_files = []
    if arch != "riscv32":
        target_dir = REPO / ".scarlet/sgfx/build" / dependency["rev"]
        env = os.environ.copy()
        env.update(CARGO_TARGET_DIR=str(target_dir), RUSTFLAGS="-Zdefault-visibility=hidden",
                   CARGO_PROFILE_RELEASE_LTO="thin", CARGO_PROFILE_RELEASE_CODEGEN_UNITS="1")
        run(["cargo", "rustc", "--locked", "--release", "--target", target,
             "-p", "sgfx-backend-scarlet-virgl-plugin", "--lib", "--",
             "-C", "link-arg=--hash-style=both", "-C", "link-arg=-z", "-C", "link-arg=now",
             "-C", "link-arg=-z", "-C", "link-arg=defs", "-C", "link-arg=--exclude-libs=ALL",
             "-C", "link-arg=--entry=0", "-C", "link-arg=-soname", "-C", "link-arg=libsgfx_scarlet_virgl.so"],
            cwd=source, env=env)
        artifact = target_dir / target / "release/libsgfx_scarlet_virgl.so"
        report = audit_driver(artifact, arch)
        library = output / artifact.name
        shutil.copy2(artifact, library)
        manifest = output / "scarlet-virgl.sgfx-driver"
        manifest.write_text("abi=2\nname=scarlet-virgl\ngpu_backend=virtio-gpu\nlibrary=libsgfx_scarlet_virgl.so\n")
        (output / "driver-elf.json").write_text(json.dumps(report, indent=2) + "\n")
        install_files = [library, manifest]
        # A shared input lets LLD keep imports supplied by scarlet-ld. --as-needed
        # removes this unreferenced backend from DT_NEEDED: discovery uses dlopen.
        flags += ["-Zdefault-visibility=hidden",
                 "-C", "link-arg=--dynamic-linker=/bin/scarlet-ld",
                 "-C", "link-arg=--as-needed", "-C", f"link-arg={library}",
                 "-C", "link-arg=--unresolved-symbols=ignore-all"]
    config = f"[target.{target}]\nrustflags = {json.dumps(flags)}\n\n[patch.{json.dumps(SGFX_URL)}]\n"
    # External native backends and UI clients still pin older coordinated core
    # sources. All SGFX crates must resolve to this single checkout.
    for package in sorted((source / "crates").glob("*/Cargo.toml")):
        name = tomllib.loads(package.read_text())["package"]["name"]
        if name == "sgfx-backend-scarlet-virgl-plugin":
            continue
        config += f"{json.dumps(name)} = {{ path = {json.dumps(str(package.parent))} }}\n"
    config_path = output / "userspace.toml"
    config_path.write_text(config)
    if args.check_client_linkage and arch != "riscv32":
        audit_client_linkage(config_path, target)
    if args.install_dir:
        args.install_dir.mkdir(parents=True, exist_ok=True)
        for path in install_files:
            shutil.copy2(path, args.install_dir / path.name)
    print(f"SGFX userspace configuration ({arch}): {output}")


if __name__ == "__main__":
    main()
