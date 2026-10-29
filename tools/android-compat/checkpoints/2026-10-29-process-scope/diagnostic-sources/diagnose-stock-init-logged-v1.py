from pathlib import Path
import importlib.util,json,shutil,sys
root=Path.cwd();sys.path.insert(0,str(root/'tools/android-compat'))
from art_inputs import sha256
spec=importlib.util.spec_from_file_location('runner',root/'tools/android-compat/run-direct.py');runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
work=root/'.build/android-services';inputs=work/'stock-init-diagnostic-inputs-v2'
record=json.loads((inputs/'init-inputs.json').read_text())
original=runner.stage_stock_inputs
output=Path(sys.argv[sys.argv.index('--output')+1])
def stage(stock,runtime,tree):
 result=original(stock,runtime,tree)
 shutil.copy2(work/'libprobe-init-kmsg.so',tree/'system/lib64/libprobe-init-kmsg.so')
 assert sha256(runtime/'art-runtime.json')==record['runtime_sha256']
 for name,digest in record['files'].items():
  source=inputs/name;target=tree/name
  assert sha256(source)==digest,name
  if target.exists():assert sha256(target)==digest,name
  else:target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,target)
 evidence={'kind':'bounded-stock-init-diagnostic-v1','init_manifest_sha256':sha256(inputs/'init-inputs.json'),
  'launcher_sha256':sha256(work/'stock-init-launcher-logged'),'launcher_source_sha256':sha256(work/'stock-init-launcher-logged.c'),'kmsg_logger_sha256':sha256(work/'libprobe-init-kmsg.so'),'kmsg_logger_source_sha256':sha256(work/'init-kmsg-logger.c'),
  'runner_source_sha256':sha256(work/'diagnose-stock-init-logged.py'),'kernel_namespace_changes':'ancestor visibility and stable parent PID mapping',
  'limits':['Diagnostic LD_PRELOAD redirects writable /dev/kmsg opens to the inherited console; no syscall result bypass','No boot-readiness assertion','No Shield init policy supplied','Existing property snapshot bootstrap retained','No namespace-init death or orphan-reaper semantics added']}
 (output/'init-diagnostic.json').write_text(json.dumps(evidence,indent=2)+'\n')
 shutil.copy2(inputs/'init-inputs.json',output/'init-inputs.json')
 return result
runner.stage_stock_inputs=stage
raise SystemExit(runner.main())
