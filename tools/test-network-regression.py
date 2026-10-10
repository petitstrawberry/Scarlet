"""Exercise production network/xHCI code with modeled DMA, IRQs and scheduling.

Compiles extracted code from this Scarlet tree, without applying patches or
changing dependency caches. Host models complement target builds; they do
not establish physical throughput or hardware coherency.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
ROOT = Path(__file__).resolve().parents[1]
HARNESS = ROOT / 'tools/network-regression'

def block(source, marker):
    if source.count(marker) != 1:
        raise ValueError(f'expected one production scope: {marker}')
    start = source.index(marker)
    end = source.index('{', start) + 1
    depth = 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def ncm_suites(c):
    paths = ['kernel/src/drivers/usb/cdc_ncm.rs', 'kernel/src/drivers/usb/xhci/mod.rs']
    ns = (c / paths[0]).read_text()
    xs = (c / paths[1]).read_text()
    constants = '\n'.join(re.findall('^(?:pub\\(crate\\) )?const (?:NTH16_\\w+|NDP16_\\w+|NCM_\\w+|ETHERNET_HEADER_LENGTH):.*;$', ns, re.M))
    packet = (c / 'kernel/src/device/network/mod.rs').read_text()
    types = block(packet, 'pub struct DevicePacket {') + '\n' + block(packet, 'impl DevicePacket {')
    ncm = constants + '\n' + types + '\n#[derive(Clone, Copy)]\n' + block(ns, 'pub(crate) struct CdcNcmParameters {') + '\n' + block(ns, 'impl CdcNcmParameters {') + '\n#[derive(Clone, Copy)]\n' + block(ns, 'struct Ntb16TxConfig {') + '\n' + block(ns, 'impl Ntb16TxConfig {')
    for marker in ['fn sanitize_alignment(', 'fn align_to_remainder(', 'pub(crate) struct Ntb16TxBatch {', 'impl Ntb16TxBatch {', 'fn build_ntb16(', 'fn parse_ntb16(', 'fn read_u16(', 'fn read_u32(', 'fn write_u16(', 'fn write_u32(']:
        ncm += '\n' + block(ns, marker)
    rxmethod = block(xs, '    fn handle_transfer_event(')
    prefix = rxmethod[:rxmethod.index('        if let Some(ncm)')]
    rx = block(rxmethod, '        if let Some(ncm) = slot.cdc_ncm.as_mut()\n            && ncm.bulk_in.dci == endpoint_id')
    tx = block(rxmethod, '        if let Some(ncm) = slot.cdc_ncm.as_mut()\n            && ncm.bulk_out.dci == endpoint_id')
    ring = (c / 'kernel/src/drivers/usb/xhci/ring.rs').read_text()
    trb = (c / 'kernel/src/drivers/usb/xhci/trb.rs').read_text()
    controller = trb[:trb.index('#[cfg(test)]')] + '\n'
    for marker in ['struct CdcNcmDmaBuffer {', 'struct InFlightCdcNcmRx {', 'struct CompletedCdcNcmRx {', 'struct InFlightCdcNcmTx {', 'fn sync_pages_before_device_write(', 'fn sync_pages_after_device_write(']:
        controller += block(xs, marker) + '\n'
    controller += block(ring, 'pub struct DmaTrbRing {') + '\n' + block(ring, 'impl DmaTrbRing {') + '\nimpl XhciController {\n' + prefix + tx + rx + '\nfalse\n}\n'
    for marker in ['    fn transfer_successful(', '    fn complete_cdc_ncm_rx(', '    fn enqueue_cdc_ncm_tx(', '    fn take_pending_cdc_ncm_tx(', '    fn restore_cdc_ncm_tx_buffer(', '    fn process_pending_cdc_ncm_tx(']:
        controller += block(xs, marker) + '\n'
    controller += '}\n'
    queue = '\n'.join((block(ns, m) for m in ['struct QueuedRxPacket {', 'fn enqueue_rx_packets(', 'fn drain_queued_rx_packets(']))
    return [('ncm', ncm, '/* PRODUCTION_NCM */', 'ncm-dma-host-tests.rs', queue), ('ownership', ncm + '\n' + controller, '/* PRODUCTION_PATHS */', 'ncm-dma-ownership-host-tests.rs', queue)]

def packet_code(sources, candidate):
    s = sources['protocol_stack.rs']
    start = s.index('// Protocol-neutral inline metadata.') if candidate else s.index('#[derive(Debug, Clone, Default)]\npub struct LayerContext')
    result = 'use alloc::{collections::BTreeMap,string::String,vec::Vec};\n' + s[start:s.index('/// Configuration for socket creation', start)]
    result += '\nconst TCP_HEADER_SIZE:usize=20;\n'
    for file, ty, methods in [('tcp.rs', 'TcpHeader', ['calculate_checksum_with_options', 'to_bytes']), ('ipv4.rs', 'Ipv4Header', ['new', 'calculate_checksum', 'to_bytes'])]:
        source = sources[file]
        impl = block(source, f'impl {ty} {{')
        result += '#[derive(Debug,Clone,Copy)]\n#[repr(C,packed)]\n' + block(source, f'pub struct {ty} {{') + '\nimpl ' + ty + ' {\n'
        for name in methods + (['to_array'] if candidate else []):
            marker = f'    pub fn {name}(' if f'    pub fn {name}(' in impl else f'    fn {name}('
            result += block(impl, marker).replace(f'    fn {name}(', f'    pub fn {name}(') + '\n'
        result += '}\n'
    return result + block(sources['ipv4.rs'], 'fn checksum_from_bytes(')

def other_suites(c):
    names = ['tcp.rs', 'ipv4.rs', 'protocol_stack.rs']
    current = {n: (c / 'kernel/src/network' / n).read_text() for n in names}
    baseline = {n: subprocess.check_output(['git', 'show', '6fa4a4ac2c4a1b05034057b16f614736a44344b2:kernel/src/network/' + n], cwd=c, text=True) for n in names}
    packet = (HARNESS / 'network-packet-allocation-host-tests.rs').read_text()
    packet = packet.replace('/* BASELINE */', 'mod baseline {\n' + packet_code(baseline, False) + '\n}')
    packet = packet.replace('/* CANDIDATE */', 'mod candidate {\n' + packet_code(current, True) + '\n}')
    owned = (c / 'kernel/src/network/packet.rs').read_text()
    packet = packet.replace('/* OWNED_PACKET */', owned[:owned.index('#[cfg(test)]')].replace('//!', '//'))
    xhci = (c / 'kernel/src/drivers/usb/xhci/mod.rs').read_text()
    worker = re.search('^const XHCI_WORKER_PASS_BUDGET:.*;$', xhci, re.M).group(0) + '\n' + block(xhci, 'fn xhci_worker_entry()')
    tcp = current['tcp.rs']
    drain = (HARNESS / 'tcp-bulk-receive-drain-host-tests.rs').read_text()
    predicates = '\n'.join((block(tcp, 'const fn ' + n + '(') for n in ['tcp_receive_side_open', 'tcp_receive_side_eof']))
    methods = '\n'.join((block(tcp, '    pub fn ' + n + '(') for n in ['recv_data', 'recv_blocking']))
    drain = drain.replace('/* PRODUCTION_STATE_PREDICATES */', predicates).replace('/* PRODUCTION_DRAIN_HELPER */', block(tcp, 'fn drain_recv_buffer(')).replace('    /* PRODUCTION_RECEIVE_METHODS */', methods)
    return [('packet', packet, None, 'network-packet-allocation-host-tests.rs', ''), ('worker', worker, '/* PRODUCTION_WORKER */', 'xhci-worker-batching-host-tests.rs', ''), ('drain', drain, None, 'tcp-bulk-receive-drain-host-tests.rs', '')]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / '.cache/network-regression')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    rustc = shutil.which('rustc')
    version = subprocess.check_output([rustc, '-vV'], text=True)
    host = re.search('^host: (.+)$', version, re.M)[1]
    paths = ['kernel/src/drivers/usb/cdc_ncm.rs', 'kernel/src/drivers/usb/xhci/mod.rs', 'kernel/src/drivers/usb/xhci/ring.rs'] + ['kernel/src/network/' + n for n in ['tcp.rs', 'ipv4.rs', 'protocol_stack.rs']]
    paths.append('kernel/src/network/packet.rs')
    before = {n: sha(ROOT / n) for n in paths}
    receipt = {'source_sha256': before, 'runs': [], 'physical_tested': False}
    with tempfile.TemporaryDirectory(prefix='network-regression-') as temp:
        for name, production, marker, harness, queue in ncm_suites(ROOT) + other_suites(ROOT):
            source = (HARNESS / harness).read_text().replace(marker, production) if marker else production
            source = source.replace('/* PRODUCTION_RX_QUEUE */', queue)
            path = output / (name + '.rs')
            path.write_text(source)
            binary = Path(temp) / name
            compiled = subprocess.run([rustc, '--edition=2024', '--target', host, '-C', 'opt-level=2', '--test', str(path), '-o', str(binary)], capture_output=True, text=True)
            (output / (name + '-compile.log')).write_text(compiled.stdout + compiled.stderr)
            compiled.check_returncode()
            tested = subprocess.run([str(binary), '--test-threads=1', '--nocapture'], capture_output=True, text=True, timeout=30)
            (output / (name + '-tests.log')).write_text(tested.stdout + tested.stderr)
            tested.check_returncode()
            result = tested.stdout.split('test result: ')[-1].strip()
            print(name + ': ' + result)
            receipt['runs'].append({'suite': name, 'result': result, 'harness_sha256': sha(HARNESS / harness), 'extracted_sha256': sha(path)})
    assert before == {n: sha(ROOT / n) for n in paths}
    receipt.update(result='pass', source_unchanged=True, temporary_binaries_removed=True)
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
if __name__ == '__main__':
    main()
