from pathlib import Path
import importlib.util,json,shutil,sys
root=Path.cwd();sys.path.insert(0,str(root/'tools/android-compat'))
from art_inputs import sha256
from framework_inputs import stage_framework_inputs
spec=importlib.util.spec_from_file_location('runner',root/'tools/android-compat/run-direct.py')
runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
work=root/'.build/android-services';framework=work/'framework-apk-inputs'
original=runner.stage_stock_inputs
output=Path(sys.argv[sys.argv.index('--output')+1])
def stage(stock,runtime,tree):
 result=original(stock,runtime,tree)
 bootstrap=stage_framework_inputs(framework,runtime,stock,tree)
 shutil.copy2(work/'zygote-child-dex/classes.dex',tree/'data/local/tmp/zygote-child.dex')
 logger=work/'libframework-exit-logger.so'
 shutil.copy2(logger,tree/'system/lib64/libprobe-stderr-logger.so')
 bootstrap['diagnostic_preload_sha256']=sha256(logger)
 bootstrap['framework_manifest_sha256']=sha256(framework/'framework-inputs.json')
 bootstrap['bootstrap_sources']={name:sha256(work/name) for name in ('zygote-child-probe.c','zygote-child-config.h','diagnose-zygote-child.py')}
 (output/'zygote-diagnostic.json').write_text(json.dumps(bootstrap,indent=2)+'\n')
 return result
runner.stage_stock_inputs=stage
raise SystemExit(runner.main())
