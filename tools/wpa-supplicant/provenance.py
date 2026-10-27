"""Pinned upstream verification and source/artifact receipts for the WPA runners."""
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
LIB = ROOT / 'user/lib/scarlet-wpa'


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def upstream():
    pin = json.loads((LIB / 'UPSTREAM.json').read_text())
    vendor = LIB / ('vendor/wpa_supplicant-' + pin['version'])
    actual = {str(p.relative_to(vendor)): sha256(p) for p in vendor.rglob('*') if p.is_file()}
    if actual != pin['files']:
        raise RuntimeError('vendored wpa_supplicant differs from UPSTREAM.json')
    for source in (LIB / 'sources.txt').read_text().splitlines():
        if source and 'src/' + source not in actual:
            raise RuntimeError('compiled source absent from upstream manifest: ' + source)
    return {'version': pin['version'], 'url': pin['url'], 'archive_sha256': pin['archive_sha256'],
            'manifest_sha256': sha256(LIB / 'UPSTREAM.json'), 'verified_files': len(actual)}


def sources():
    # Capture production kernel/runtime inputs as well as both independent peers.
    roots = ('kernel', 'user/lib/scarlet-wpa', 'user/lib/scarlet-abi',
             'user/lib/scarlet-os', 'user/lib/scarlet-sys', 'user/lib/scarlet-rt',
             'user/lib/std', 'user/bin', 'user/scarlet-ld', 'user/wpa-supplicant',
             'tools/wpa-supplicant', 'tools/wireless-ccmp', 'tools/network-management/guest')
    files = set()
    for root in roots:
        for path in (ROOT / root).rglob('*'):
            if not path.is_file() or {'target', '__pycache__', '.git'} & set(path.parts):
                continue
            if path.suffix in ('.rs', '.c', '.h', '.toml', '.lock', '.json', '.ld', '.py', '.txt'):
                files.add(path)
    files.update(ROOT / p for p in ('flake.nix', 'flake.lock', 'rust-toolchain.toml',
                                   '.cargo/config.toml', 'tools/test-network-management.py',
                                   'tools/test-wireless-data.py', 'tools/test-wpa-supplicant.py',
                                   'tools/test-wpa-supplicant-host.py', 'tools/test-wireless-ccmp.py', 'tools/test-wireless-rekey.py'))
    return {str(p.relative_to(ROOT)): sha256(p) for p in sorted(files) if p.is_file()}


def toolchain(qemu=False):
    result = {'rustc': subprocess.check_output(['rustc', '-vV'], text=True).strip(),
              'openssl': subprocess.check_output(['openssl', 'version'], text=True).strip()}
    if qemu:
        result['qemu'] = subprocess.check_output(['qemu-system-aarch64', '--version'], text=True).splitlines()[0]
    return result
