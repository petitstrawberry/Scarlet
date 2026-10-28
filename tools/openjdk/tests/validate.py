#!/usr/bin/env python3
"""Build an isolated Scarlet guest and execute the native OpenJDK smoke test.

Run in nix develop, after build.py --target java. Outputs stay in a dedicated
owned directory. No existing disk image or host virtual machine is modified.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
OWNER = 'scarlet-openjdk-guest-v1\n'

def run(command, log, env):
    print('Running '+str(command[0])+'; log: '+str(log), flush=True)
    with log.open('w') as stream:
        result = subprocess.run(list(map(str, command)), cwd=ROOT, env=env,
                                stdout=stream, stderr=subprocess.STDOUT)
    if result.returncode:
        print(log.read_text(errors='replace')[-5000:])
        raise SystemExit(f'Failed: {log}')

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--build-receipt', type=Path, required=True)
    p.add_argument('--boot-jdk', type=Path, required=True)
    p.add_argument('--output', type=Path, default=ROOT/'.build/openjdk-guest')
    p.add_argument('--accel', choices=('tcg','hvf'), default='tcg')
    p.add_argument('--timeout', type=int, default=180)
    a = p.parse_args()
    receipt = json.loads(a.build_receipt.read_text())
    a.jit = receipt.get('inputs', {}).get('jvm_variant', 'zero') != 'zero'
    java, java_home = Path(receipt['artifact']), Path(receipt['java_home'])
    if not java.is_file() or not (java_home/'lib/modules').is_file():
        p.error('Receipt must point to a compiled native Java runtime')
    if a.output.is_symlink(): p.error('Output must not be a symlink')
    o = a.output.absolute()
    o.mkdir(parents=True, exist_ok=True)
    marker = o/'.scarlet-openjdk-guest'
    if marker.exists():
        if marker.is_symlink() or marker.read_text() != OWNER: p.error('Unrecognized output owner')
    else:
        if any(o.iterdir()): p.error('Output must be empty or owned by this helper')
        marker.write_text(OWNER)
    import fcntl
    with marker.open() as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        build_and_run(a, o, java, java_home)

def build_and_run(a, o, java, java_home):
    kernel = o/'kernel'
    (kernel/'src').mkdir(parents=True, exist_ok=True)
    (kernel/'Cargo.toml').write_text('''[package]
name = "openjdk-qa-kernel"
version = "0.1.0"
edition = "2024"
[workspace]
[dependencies]
scarlet = { path = '''+json.dumps(str(ROOT/'kernel'))+''', default-features = false, features = ["linux-boot", "network", "user-fpu", "user-vector"] }
[profile.release]
panic = "abort"
''')
    (kernel/'src/main.rs').write_text('''#![no_std]
#![no_main]
#[used]
static ENTRY: extern "C" fn() -> ! = scarlet::arch::aarch64::boot::linux::image_head;
''')
    target = json.loads((ROOT/'kernel/targets/aarch64-unknown-none-elf.json').read_text())
    target.pop('pre-link-args', None)
    target_file = kernel/'aarch64-openjdk-none.json'
    target_file.write_text(json.dumps(target)+'\n')
    env = os.environ.copy()
    env.pop('RUSTFLAGS', None)
    env['CARGO_TARGET_DIR'] = str(kernel/'target')
    env['CARGO_ENCODED_RUSTFLAGS'] = '-Clink-arg=-T'+str(ROOT/'tools/network-management/guest/kernel.ld')
    run(['cargo','build','--manifest-path',kernel/'Cargo.toml','--release','--target',target_file,
         '-Z','build-std=core,alloc,compiler_builtins'],o/'kernel-build.log',env)
    env.pop('CARGO_ENCODED_RUSTFLAGS')
    run(['llvm-objcopy','-O','binary',kernel/'target/aarch64-openjdk-none/release/openjdk-qa-kernel',o/'Image'],o/'objcopy.log',env)
    env['CARGO_TARGET_DIR'] = str(o/'userspace-target')
    run(['cargo','build','--manifest-path',ROOT/'user/bin/Cargo.toml','--no-default-features',
         '--bin','init','--release','--target','aarch64-unknown-scarlet'],o/'init-build.log',env)
    run(['rustc','--edition=2024','--target=aarch64-unknown-scarlet','-C','panic=abort',
         '-C','target-feature=+crt-static','-C','relocation-model=static',
         '-C','link-arg=--no-dynamic-linker','-O',HERE/'supervisor.rs','-o',o/'supervisor']
        + (['--cfg','jit'] if a.jit else []),o/'supervisor-build.log',env)
    run([a.boot_jdk.resolve()/'bin/javac','-encoding','UTF-8','-d',o/'classes',HERE/'Hello.java',HERE/'Jit.java'],o/'javac.log',env)
    run([sys.executable,HERE/'run-java.py','--kernel',o/'Image','--init',
         o/'userspace-target/aarch64-unknown-scarlet/release/init','--java',java,
         '--java-home',java_home,'--supervisor',o/'supervisor','--classes',o/'classes',
         '--output',o/'java-qa','--accel',a.accel,'--timeout',a.timeout]
        + (['--jit'] if a.jit else []),o/'run.log',env)
    print('Native OpenJDK guest PASS: '+str(o/'java-qa/validation.json'))

if __name__ == '__main__':
    main()
