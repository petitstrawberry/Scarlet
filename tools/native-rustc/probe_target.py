#!/usr/bin/env python3
"""Cross-compile bring-up probes; record actual compiler exits, including blockers.

This does not execute Scarlet binaries and does not claim native rustc works.
Exit 1 means a probe failed. All phases are attempted and logged separately.
"""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

TARGETS = ("riscv64gc-unknown-scarlet", "aarch64-unknown-scarlet")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--rustc", required=True)
    p.add_argument("--target", choices=TARGETS, default=TARGETS[0])
    p.add_argument("--output", type=Path, required=True, help="New directory, never overwritten")
    p.add_argument("--libloading-source", type=Path, help="Optional unpacked libloading 0.8.9 source")
    args = p.parse_args()
    rustc = shutil.which(args.rustc)
    if rustc is None:
        p.error("rustc is not executable")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    results = []

    def run(name, extra, artifact=None):
        command = [rustc, *map(str, extra)]
        (output / (name + ".command.json")).write_text(json.dumps(command, indent=2) + "\n")
        with (output / (name + ".stdout")).open("w") as stdout, (output / (name + ".stderr")).open("w") as stderr:
            try:
                status = subprocess.run(command, cwd=output, stdout=stdout, stderr=stderr, timeout=180).returncode
            except subprocess.TimeoutExpired:
                status = 124
        produced = artifact is None or (output / artifact).is_file()
        passed = status == 0 and produced
        result = {"phase": name, "exit": status, "passed": passed}
        if artifact:
            result["artifact"] = artifact
            result["artifact_exists"] = produced
        results.append(result)
        print(f"{name}: exit {status}, {'PASS' if passed else 'FAIL'}")
        print((output / (name + ".stderr")).read_text(), file=sys.stderr, end="")
        if not produced:
            print(f"{name}: compiler did not produce {artifact}", file=sys.stderr)
        return 0 if passed else 1

    run("compiler-version", ["-Vv"])
    run("target-spec", ["-Zunstable-options", "--print", "target-spec-json", "--target", args.target])
    run("target-cfg", ["--print", "cfg", "--target", args.target])
    common = ["--edition=2021", "--target", args.target, "-Cpanic=abort"]
    (output / "hello.rs").write_text('fn main() { println!("native Scarlet std probe"); }\n')
    (output / "dylib.rs").write_text('pub fn hello() -> String { "Scarlet Rust dylib".into() }\n')
    (output / "cdylib.rs").write_text('''#![no_std]
#[panic_handler] fn panic(_: &core::panic::PanicInfo<'_>) -> ! { loop {} }
#[no_mangle] pub extern "C" fn scarlet_probe() -> u64 { 42 }
''')
    run("static-std", [*common, "hello.rs", "-o", "hello"], "hello")
    run("cdylib", [*common, "--crate-type=cdylib", "cdylib.rs", "-o", "libprobe-c.so"], "libprobe-c.so")
    run("rust-dylib", [*common, "--crate-type=dylib", "dylib.rs", "-o", "libprobe-rust.so"], "libprobe-rust.so")
    run("guest-probe", [*common, Path(__file__).with_name("probe.rs").resolve(), "-o", "native-rustc-probe"], "native-rustc-probe")
    if args.libloading_source:
        source = args.libloading_source.resolve() / "src/lib.rs"
        # On the current non-Unix Scarlet target the upstream crate has no platform
        # implementation or cfg-if use. A future backend may require Cargo instead.
        status = run("libloading-build", [*common, "--crate-name=libloading", "--crate-type=rlib", source,
                                         "-o", "libloading.rlib"], "libloading.rlib")
        if status == 0:
            (output / "libloading_api.rs").write_text("use libloading::Library;\nfn main() { let _ = core::mem::size_of::<Library>(); }\n")
            run("libloading-api", [*common, "--emit=metadata", "libloading_api.rs", "--extern", "libloading=libloading.rlib"])
    (output / "results.json").write_text(json.dumps({"target": args.target, "rustc": rustc,
        "executed_on_scarlet": False, "phases": results}, indent=2) + "\n")
    return int(any(not result["passed"] for result in results))


if __name__ == "__main__":
    sys.exit(main())
