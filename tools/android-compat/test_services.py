import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from art_inputs import sha256
from evidence import analyze_services
from libbinder_inputs import SOURCE, verify_libbinder_build
from service_inputs import CONTEXTS, verify_service_inputs, stage_service_inputs
from test_libbinder import build_fixture as binder_fixture
from test_art_runner import runner, stock_fixture


def successful_log():
    return '\n' + '\n'.join([
        'SCARLET_SERVICES_CONFIGURED',
        'SCARLET_ANDROID_STOCK_START properties', 'SCARLET_PROPERTY_SNAPSHOT_OK 216',
        'SCARLET_ANDROID_STOCK_EXIT properties 0', 'SCARLET_ANDROID_STOCK_START linkerconfig',
        'SCARLET_ANDROID_STOCK_EXIT linkerconfig 0', 'SCARLET_ANDROID_STOCK_OK',
        'SCARLET_SERVICES_SPAWN manager 2', 'SCARLET_SERVICES_SPAWN owner 3',
        'SCARLET_SERVICES_SPAWN client 4', 'SCARLET_SERVICES_SPAWN denied 5',
        'SCARLET_SERVICES_PEER owner', 'SCARLET_SERVICES_PEER client', 'SCARLET_SERVICES_PEER denied',
        'SCARLET_SERVICES_REGISTERED', 'SCARLET_SERVICES_LOOKUP_CALL_LIST_OK',
        'SCARLET_SERVICES_DENIED_ADD_FIND_LIST', 'SCARLET_SERVICES_OWNER_EXITING',
        'SCARLET_SERVICES_EXIT owner 0', 'SCARLET_SERVICES_OWNER_DEATH_OK',
        'SCARLET_SERVICES_EXIT client 0', 'SCARLET_SERVICES_EXIT denied 0',
        'SCARLET_SERVICES_EXIT manager 137', 'SCARLET_SERVICES_OK']) + '\n'


def fixture(base):
    runtime, build, info = binder_fixture(base)
    lib = runtime / 'system/lib64/libselinux.so'
    lib.write_bytes(b'simulated libselinux')
    manifest = json.loads((runtime / 'art-runtime.json').read_text())
    manifest['files']['system/lib64/libselinux.so'] = sha256(lib)
    (runtime / 'art-runtime.json').write_text(json.dumps(manifest))
    (build / 'libbinder-probe').rename(build / 'service-probe')
    info.update(kind='stock-service-probe-v1', source_sha256=sha256(SOURCE / 'service-probe.cpp'),
                runtime_manifest_sha256=sha256(runtime / 'art-runtime.json'))
    (build / 'build.json').write_text(json.dumps(info))
    services = base / 'services'
    program = services / 'system/bin/servicemanager'
    program.parent.mkdir(parents=True); program.write_bytes(b'simulated manager')
    metadata = {'result':'PREPARED', 'base_manifest_sha256':sha256(runtime / 'art-runtime.json'),
                'runtime_libraries':{'system/lib64/libselinux.so':sha256(lib)},
                'files':{'system/bin/servicemanager':sha256(program)}}
    (services / 'service-inputs.json').write_text(json.dumps(metadata))
    return runtime, build, services


class ServicesTest(unittest.TestCase):
    def test_evidence_requires_authorization_lifecycle_and_owned_shutdown(self):
        text = successful_log(); lines = text.strip().splitlines()
        self.assertEqual(analyze_services(text.encode())['result'], 'PASS')
        for i in range(len(lines)):
            with self.subTest(missing=lines[i]):
                self.assertEqual(analyze_services('\n'.join(lines[:i]+lines[i+1:]).encode())['result'],'INCOMPLETE')
        for broken in [text.replace('EXIT manager 137','EXIT manager 0'),
                       text.replace('EXIT owner 0','EXIT owner 1'),
                       text.replace('SPAWN denied 5','SPAWN denied 4'),
                       text.replace('SCARLET_SERVICES_OK','SCARLET_SERVICES_FAIL\nSCARLET_SERVICES_OK'),
                       text.replace('OWNER_EXITING','OWNER_DEATH_OK'), text + 'SCARLET_SERVICES_OK\n']:
            self.assertEqual(analyze_services(broken.encode())['result'],'INCOMPLETE')
        # Authorized and denied clients may complete in either order.
        self.assertEqual(analyze_services(text.replace('LOOKUP_CALL_LIST_OK','SWAP').replace('DENIED_ADD_FIND_LIST','LOOKUP_CALL_LIST_OK').replace('SWAP','DENIED_ADD_FIND_LIST').encode())['result'],'PASS')

    def test_verified_manager_and_policy_staging_reject_stale_inputs_and_overwrites(self):
        with tempfile.TemporaryDirectory() as temporary:
            base=Path(temporary);runtime,build,services=fixture(base)
            self.assertEqual(verify_libbinder_build(build,runtime,service=True),build/'service-probe')
            with self.assertRaises(ValueError):verify_libbinder_build(build,runtime)
            program=verify_service_inputs(services,runtime)
            tree=base/'tree';(tree/'system/bin').mkdir(parents=True)
            stage_service_inputs(services,runtime,tree)
            self.assertEqual((tree/'system/bin/servicemanager').read_bytes(),program.read_bytes())
            self.assertEqual((tree/'system/etc/selinux/plat_service_contexts').read_text(),CONTEXTS)
            with self.assertRaises(ValueError):stage_service_inputs(services,runtime,tree)
            program.write_bytes(b'changed manager')
            with self.assertRaises(ValueError):verify_service_inputs(services,runtime)

    def test_runner_preserves_failure_and_timeout_and_binds_policy_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            base=Path(temporary);runtime,build,services=fixture(base)
            stock=base/'stock';stock_fixture(stock,runtime)
            kernel,firmware=base/'kernel',base/'firmware'
            kernel.write_bytes(b'kernel');firmware.write_bytes(b'firmware')
            for variant in ('complete','missing','timeout'):
                output=base/variant
                def compile(command,**kwargs):
                    if command[0]=='cargo':
                        self.assertEqual(kwargs['env']['SCARLET_ANDROID_SERVICES'],'1')
                        target=output/'target/aarch64-unknown-scarlet/release';target.mkdir(parents=True,exist_ok=True)
                        for name in ('android-direct-init','scarlet-ld'):(target/name).write_bytes(b'simulated launcher')
                def guest(command,destination,timeout):
                    self.assertEqual(command[command.index('-accel')+1],'hvf')
                    data=successful_log()
                    if variant=='missing':data=data.replace('SCARLET_SERVICES_DENIED_ADD_FIND_LIST\n','')
                    (destination/'serial.log').write_text(data)
                    return {'result':'timeout' if variant=='timeout' else 'PASS'}
                args=['run-direct.py','--kernel',str(kernel),'--service-probe',str(build),'--service-inputs',str(services),
                      '--runtime-root',str(runtime),'--stock-inputs',str(stock),'--accel','hvf','--output',str(output)]
                with patch('sys.argv',args),patch.object(runner.smoke,'firmware',return_value=firmware),\
                     patch.object(runner.subprocess,'run',side_effect=compile),patch.object(runner.smoke,'run_guest',side_effect=guest):
                    self.assertEqual(runner.main(),0 if variant=='complete' else 1)
                result=json.loads((output/'result.json').read_text())
                self.assertEqual(result['result']=='PASS',variant=='complete')
                if variant=='timeout':self.assertEqual(result['result'],'timeout')
                inputs=json.loads((output/'inputs.json').read_text())
                self.assertEqual(inputs['service_fixture_sha256'],sha256(SOURCE/'init/src/services.rs'))
                self.assertEqual((output/'service-inputs.json').read_bytes(),(services/'service-inputs.json').read_bytes())

if __name__=='__main__':unittest.main()
