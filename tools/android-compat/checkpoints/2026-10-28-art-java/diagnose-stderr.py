from pathlib import Path
import hashlib, importlib.util, json, shutil, sys
root = Path.cwd()
sys.path.insert(0, str(root / "tools/android-compat"))
spec = importlib.util.spec_from_file_location("runner", root / "tools/android-compat/run-direct.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
launch, stage = runner.art_launch, runner.stage_stock_inputs
logger = root / ".build/android-art-next/libart-stderr-logger.so"
def diagnostic_launch(metadata):
    result = launch(metadata)
    result["env"].append("LD_PRELOAD=/apex/com.android.art/lib64/libart-stderr-logger.so")
    return result
def diagnostic_stage(stock, runtime, tree):
    result = stage(stock, runtime, tree)
    shutil.copy2(logger, tree / "apex/com.android.art/lib64/libart-stderr-logger.so")
    (tree.parents[1].parent / "diagnostic-logger.json").write_text(json.dumps({"purpose": "diagnostic log sink via Android public logging API", "sha256": hashlib.sha256(logger.read_bytes()).hexdigest()}))
    return result
runner.art_launch, runner.stage_stock_inputs = diagnostic_launch, diagnostic_stage
raise SystemExit(runner.main())
