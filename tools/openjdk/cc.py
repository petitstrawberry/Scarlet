#!/usr/bin/env python3
"""Clang driver for the SDK's native AArch64 C/C++ runtime (no host headers)."""
import os
from pathlib import Path
import subprocess
import sys

args = sys.argv[1:]
cxx = Path(sys.argv[0]).name.endswith('++')
cc = os.environ['SCARLET_NATIVE_CXX' if cxx else 'SCARLET_NATIVE_CC']
runtime = Path(os.environ['SCARLET_NATIVE_RUNTIME'])
resource = subprocess.check_output([cc, '-print-resource-dir'], text=True).strip()
flags = ['--target=aarch64-unknown-none-elf', '-nostdinc', '-isystem', resource+'/include',
         '-isystem', str(runtime/'include'), '-D__scarlet__=1', '-DSSIZE_MAX=LONG_MAX',
         '-fno-stack-protector', '-mno-outline-atomics']
if cxx:
    flags = ['-nostdinc++', '-isystem', str(runtime/'include/c++/v1'), '-fno-exceptions', '-fno-rtti'] + flags
linking = not any(a in args for a in ('-c', '-E', '-S', '--version', '-v', '-dumpmachine', '-dumpversion', '-print-search-dirs'))
# These APIs live in libscarlet_c, not separate Unix libraries.
args = [a for a in args if a not in ('-lm', '-ldl', '-lpthread', '-lc', '-lstdc++', '-lc++')]
if linking:
    rustc = os.environ.get('SCARLET_RUSTC', 'rustc')
    sysroot = Path(subprocess.check_output([rustc, '--print', 'sysroot'], text=True).strip())
    crt = sysroot/'lib/rustlib/aarch64-unknown-scarlet/lib/scarlet-crt0.o'
    if not crt.is_file():
        raise SystemExit('Scarlet Rust CRT not found: '+str(crt))
    flags += ['-nostdlib', '-fuse-ld=lld', '-static', '-Wl,--gc-sections',
              '-Wl,-z,max-page-size=4096', '-Wl,--wrap=main', '-Wl,--undefined=__wrap_main', str(crt)]
    args += ['-Wl,--start-group'] + [str(runtime/'lib'/lib) for lib in
             ('libc++.a', 'libc++abi.a', 'libscarlet_c_startup.a', 'libscarlet_cxx_support.a', 'libscarlet_c.a')] + ['-Wl,--end-group']
environment = os.environ.copy()
# -nostdinc does not suppress environment-supplied include directories.
for name in ('CPATH', 'C_INCLUDE_PATH', 'CPLUS_INCLUDE_PATH', 'OBJC_INCLUDE_PATH', 'LIBRARY_PATH'):
    environment.pop(name, None)
result = subprocess.run([cc, *flags, *args], env=environment)
if result.returncode == 0 and linking:
    out = Path(args[args.index('-o')+1]) if '-o' in args else Path('a.out')
    if out.is_file():
        with out.open('r+b') as file:
            if file.read(4) != b'\x7fELF':
                raise SystemExit('Cross compiler did not emit ELF')
            file.seek(7)
            file.write(b'\x53')
sys.exit(result.returncode)
