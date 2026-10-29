import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from art_inputs import sha256
from evidence import analyze_selinux
from selinux_inputs import SOURCE,LIBRARY,verify_selinux_build
from test_art_runner import runtime_fixture,stock_fixture,runner


def successful_log():
    return '\n'+'\n'.join([
        'SCARLET_SHIELD_CONFIGURED',
        'SCARLET_ANDROID_STOCK_START properties','SCARLET_PROPERTY_SNAPSHOT_OK 216',
        'SCARLET_ANDROID_STOCK_EXIT properties 0','SCARLET_ANDROID_STOCK_START linkerconfig',
        'SCARLET_ANDROID_STOCK_EXIT linkerconfig 0','SCARLET_ANDROID_STOCK_OK',
        'SCARLET_ANDROID_PROGRAM_START','SCARLET_SHIELD_BEGIN',
        'SCARLET_SHIELD_RUNTIME /system/lib64/libselinux.so','SCARLET_SHIELD_STATUS_OK',
        'SCARLET_SHIELD_ALLOW_DENY_OK','SCARLET_SHIELD_UNKNOWN_DENIED',
        'SCARLET_SHIELD_MUTATION_DENIED','SCARLET_SHIELD_EXEC_LABEL_OK',
        'SCARLET_SHIELD_FORK_EXEC_OK','SCARLET_SHIELD_OK',
        'SCARLET_ANDROID_PROGRAM_EXIT 0','SCARLET_ANDROID_PROGRAM_DONE'])+'\n'


def build_fixture(base):
    runtime,build=base/'runtime',base/'build'
    runtime.mkdir();build.mkdir()
    metadata=runtime_fixture(runtime)
    library=runtime/LIBRARY
    library.parent.mkdir(parents=True,exist_ok=True);library.write_bytes(b'simulated libselinux')
    metadata['files'][LIBRARY]=sha256(library)
    (runtime/'art-runtime.json').write_text(json.dumps(metadata))
    binary=build/'selinux-probe';binary.write_bytes(b'simulated probe')
    info={'result':'BUILT','kind':'stock-selinux-probe-v1',
          'source_sha256':sha256(SOURCE/'selinux-probe.c'),'runtime_manifest_sha256':sha256(runtime/'art-runtime.json'),
          'libselinux_sha256':sha256(library),'binary_sha256':sha256(binary)}
    (build/'build.json').write_text(json.dumps(info))
    return runtime,build,info


class SelinuxTest(unittest.TestCase):
    def test_evidence_requires_all_permissions_mutation_and_label_checks(self):
        log=successful_log();lines=log.strip().splitlines()
        self.assertEqual(analyze_selinux(log.encode())['result'],'PASS')
        for i in range(len(lines)):
            self.assertEqual(analyze_selinux('\n'.join(lines[:i]+lines[i+1:]).encode())['result'],'INCOMPLETE')
        for text in [log.replace('PROGRAM_EXIT 0','PROGRAM_EXIT 1'),
                     log.replace('/system/lib64/libselinux.so','/tmp/shim.so'),
                     log.replace('SCARLET_SHIELD_OK','SCARLET_SHIELD_OK\nSCARLET_SHIELD_FAIL'),
                     log.replace('SCARLET_SHIELD_STATUS_OK','SCARLET_SHIELD_STATUS_OK\nSCARLET_SHIELD_STATUS_OK'),
                     '\n'.join(reversed(lines))]:
            self.assertEqual(analyze_selinux(text.encode())['result'],'INCOMPLETE')
    def test_build_identity_rejects_stale_source_and_runtime(self):
        with tempfile.TemporaryDirectory() as temporary:
            runtime,build,metadata=build_fixture(Path(temporary))
            self.assertEqual(verify_selinux_build(build,runtime),build/'selinux-probe')
            for key in metadata:
                changed=dict(metadata);changed[key]='wrong';(build/'build.json').write_text(json.dumps(changed))
                with self.assertRaises(ValueError):verify_selinux_build(build,runtime)
    def test_runner_requires_complete_evidence_and_records_policy_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            base=Path(temporary);runtime,build,_=build_fixture(base)
            stock=base/'stock';stock_fixture(stock,runtime)
            kernel,firmware=base/'kernel',base/'firmware'
            kernel.write_bytes(b'kernel');firmware.write_bytes(b'firmware')
            for variant in ('complete','missing','timeout'):
                output=base/variant
                def compile(command,**kwargs):
                    if command[0]=='cargo':
                        self.assertEqual(kwargs['env']['SCARLET_ANDROID_SHIELD'],'1')
                        target=output/'target/aarch64-unknown-scarlet/release';target.mkdir(parents=True,exist_ok=True)
                        for name in ('android-direct-init','scarlet-ld'):(target/name).write_bytes(b'simulated launcher')
                def guest(command,destination,timeout):
                    self.assertEqual(command[command.index('-accel')+1],'hvf')
                    data=successful_log()
                    if variant=='missing':data=data.replace('SCARLET_SHIELD_ALLOW_DENY_OK\n','')
                    (destination/'serial.log').write_text(data)
                    return {'result':'timeout' if variant=='timeout' else 'PASS'}
                args=['run-direct.py','--kernel',str(kernel),'--selinux-probe',str(build),'--runtime-root',str(runtime),
                      '--stock-inputs',str(stock),'--accel','hvf','--output',str(output)]
                with patch('sys.argv',args),patch.object(runner.smoke,'firmware',return_value=firmware),\
                        patch.object(runner.subprocess,'run',side_effect=compile),patch.object(runner.smoke,'run_guest',side_effect=guest):
                    self.assertEqual(runner.main(),0 if variant=='complete' else 1)
                result=json.loads((output/'result.json').read_text())
                self.assertEqual(result['result']=='PASS',variant=='complete')
                if variant=='timeout':self.assertEqual(result['result'],'timeout')
                self.assertEqual(result['servicemanager'],'not tested')
                inputs=json.loads((output/'inputs.json').read_text())
                self.assertEqual(inputs['shield_fixture_sha256'],sha256(SOURCE/'init/src/shield.rs'))
                self.assertEqual((output/'selinux-build.json').read_bytes(),(build/'build.json').read_bytes())

if __name__=='__main__':unittest.main()
