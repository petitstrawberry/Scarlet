from pathlib import Path
import hashlib, importlib.util, json, shutil, sys
root = Path.cwd()
sys.path.insert(0, str(root / 'tools/android-compat'))
spec = importlib.util.spec_from_file_location('runner', root / 'tools/android-compat/run-direct.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
stage, run = runner.stage_stock_inputs, runner.subprocess.run
logger = root / '.build/android-art-next/libart-stderr-logger.so'
def diagnostic_stage(stock, runtime, tree):
    result = stage(stock, runtime, tree)
    shutil.copy2(logger, tree / 'system/lib64/libprobe-stderr-logger.so')
    (tree.parents[1].parent / 'diagnostic-logger.json').write_text(json.dumps({'purpose':'diagnostic only: redirect Android logging to stderr','sha256':hashlib.sha256(logger.read_bytes()).hexdigest()}))
    return result
def diagnostic_run(command, *args, **kwargs):
    if command[0] == 'cargo' and 'env' in kwargs:
        env = kwargs['env'] = kwargs['env'].copy()
        env['SCARLET_ART_ENV'] += '\x1fLD_PRELOAD=/system/lib64/libprobe-stderr-logger.so'
        output = Path(command[command.index('--target-dir')+1]).parent
        path = output/'program-launch.json'
        metadata=json.loads(path.read_text()); metadata['env']=env['SCARLET_ART_ENV'].split('\x1f')
        path.write_text(json.dumps(metadata,indent=2)+'\n')
    return run(command, *args, **kwargs)
runner.stage_stock_inputs, runner.subprocess.run = diagnostic_stage, diagnostic_run
raise SystemExit(runner.main())
