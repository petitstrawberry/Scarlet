#!/usr/bin/env python3
"""Run the real PCI configuration implementation with host-owned config I/O."""
from pathlib import Path
import hashlib
import json
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / '.build/pci-config-host'


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    source = ROOT / 'kernel/src/device/pci/config.rs'
    with tempfile.TemporaryDirectory(prefix='source-', dir=OUT) as temporary:
        temporary = Path(temporary)
        # Kernel custom-test attributes become ordinary host tests; the actual
        # implementation and its existing decode tests remain the same source.
        (temporary / 'config.rs').write_text(source.read_text().replace('#[test_case]', '#[test]'))
        harness = (ROOT / 'tools/pci-config-host-tests.rs').read_text()
        (temporary / 'main.rs').write_text(harness)
        rustc = os.environ.get('RUSTC', 'rustc')
        with (OUT / 'build.log').open('w') as log:
            subprocess.run([rustc, '--edition=2024', '--test', str(temporary / 'main.rs'),
                            '-o', str(temporary / 'tests')], stdout=log, stderr=subprocess.STDOUT, check=True)
        with (OUT / 'tests.log').open('w') as log:
            subprocess.run([str(temporary / 'tests')], stdout=log, stderr=subprocess.STDOUT, check=True)
    (OUT / 'result.json').write_text(json.dumps({'pass': True,
        'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
        'host_routing': True, 'subword_widths': True, 'invalid_access_rejected': True}, indent=2) + '\n')
    print((OUT / 'tests.log').read_text())


if __name__ == '__main__':
    main()
