#!/usr/bin/env python3
"""Execute the actual native HotSpot VM and Java tests in a disposable VM."""
import argparse
import hashlib
import importlib.util
import json
import re
from pathlib import Path
import subprocess
import sys

spec = importlib.util.spec_from_file_location('run_runtime', Path(__file__).with_name('run-runtime.py'))
runtime_test = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime_test)

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def compiled_method(log, owner, method):
    # Multiple Java threads can interleave PrintCompilation and exclusion
    # messages. Require the timestamp/id, blocking flag and byte count, but
    # allow a real compilation record to follow another writer's line prefix.
    return bool(re.search(r'(?<!\d)\d+[ \t]+\d+[ \t]+[^\r\n]*\bb[ \t]+'
                          + re.escape(owner+'::'+method) + r' \(\d+ bytes\)', log))

def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('kernel', 'init', 'java', 'java-home', 'supervisor', 'classes', 'output'):
        p.add_argument('--'+name, type=Path, required=True)
    p.add_argument('--accel', choices=('tcg', 'hvf'), default='tcg')
    p.add_argument('--cpus', type=int, default=4)
    p.add_argument('--timeout', type=int, default=180)
    p.add_argument('--jit', action='store_true', help='Require successful C1 compilation and exception recovery')
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=True)
    if not re.fullmatch(r'openjdk-(21|25)', a.java_home.name):
        p.error('Java home must be an openjdk-21 or openjdk-25 image directory')
    guest_home = 'usr/lib/jvm/' + a.java_home.name
    entries = {n:(0o40755,b'') for n in ('bin','dev','etc','tmp','root','mnt','mnt/newroot','usr','usr/lib','usr/lib/jvm',guest_home,'app')}
    inputs = {}
    def file(path, name, executable=False):
        if path.is_symlink() or not path.is_file():
            raise ValueError('Expected regular input: '+str(path))
        entries[name] = (0o100755 if executable else 0o100644, path.read_bytes())
        inputs[name] = sha(path)
    file(a.init, 'init', True)
    file(a.java, 'bin/java', True)
    file(a.supervisor, 'bin/java-test', True)
    for base, dest in ((a.java_home, guest_home), (a.classes, 'app')):
        if not base.is_dir(): raise ValueError('Missing directory: '+str(base))
        for path in sorted(base.rglob('*')):
            name = dest+'/'+path.relative_to(base).as_posix()
            if path.is_symlink(): raise ValueError('Input symlink is unsupported: '+str(path))
            if path.is_dir(): entries[name] = (0o40755, b'')
            else: file(path, name)
    initrd = a.output/'initramfs.cpio'
    runtime_test.archive([(name,*value) for name,value in sorted(entries.items())], initrd)
    command = ['qemu-system-aarch64','-machine','virt,gic-version=3','-accel',a.accel,
        '-cpu','host' if a.accel=='hvf' else 'max','-m','1G','-smp',str(a.cpus),
        '-display','none','-monitor','none','-serial','stdio','-no-reboot',
        '-kernel',str(a.kernel),'-initrd',str(initrd),
        '-append','console=ttyAMA0 init.exec=/bin/java-test']
    log = a.output/'qemu.log'
    timed_out = False
    with log.open('w') as stream:
        try:
            result = subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT, timeout=a.timeout)
            status = result.returncode
        except subprocess.TimeoutExpired:
            timed_out, status = True, None
    content = log.read_text(errors='replace')
    passed = (status == 0 and not timed_out and 'OPENJDK_GUEST FAIL' not in content
        and 'OPENJDK_JAVA PASS threads=4 atomic=4000 allocated=128MiB files=ok' in content
        and 'OPENJDK_GUEST PASS child exit=0' in content
        and 'Hello from OpenJDK on Scarlet!' in content)
    if a.jit:
        stress = content.partition('OPENJDK_GUEST JIT stress start')[2]
        passed = (passed and 'OPENJDK_JIT PASS arithmetic=ok null=ok stack=ok' in content
            and 'OPENJDK_GUEST JIT child exit=0' in content
            and 'OPENJDK_JIT_STRESS PASS threads=4 gc=8 null=32 stack=4 roots=ok' in content
            and 'OPENJDK_GUEST JIT stress child exit=0' in content
            and len(re.findall(r'Pause Full \(System.gc\(\)\)', stress)) == 8
            and all(compiled_method(stress, 'JitStress', name)
                    for name in ('round', 'field', 'recurse'))
            and all(compiled_method(content, 'Jit', name)
                    for name in ('work', 'load', 'recurse')))
    receipt = dict(result='PASS' if passed else 'FAIL', scope='Native OpenJDK Java execution',
        command=command, timed_out=timed_out, qemu_exit=status, kernel_sha256=sha(a.kernel),
        input_sha256=inputs, initramfs_sha256=sha(initrd), log_sha256=sha(log))
    receipt['jit_required'] = a.jit
    (a.output/'validation.json').write_text(json.dumps(receipt, indent=2)+'\n')
    print(content[-6000:])
    if not passed: raise SystemExit('Java guest validation failed; see '+str(log))
    print('Native OpenJDK Java execution PASS')

if __name__ == '__main__':
    main()
