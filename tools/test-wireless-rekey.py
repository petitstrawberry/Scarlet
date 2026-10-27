#!/usr/bin/env python3
"""Protected EAPOL and confirmed GTK rotation through the actual Native daemon."""
import importlib.util
from pathlib import Path
import sys
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('ccmp_runner',ROOT/'tools/test-wireless-ccmp.py')
runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
runner.OUT=ROOT/'.build/wireless-rekey';runner.network.OUT=runner.OUT
if __name__=='__main__':runner.main(rekey=True)
