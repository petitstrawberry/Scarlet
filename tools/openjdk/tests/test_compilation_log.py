"""Require actual compilation records even when native output is interleaved."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("run_java", Path(__file__).with_name("run-java.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class CompilationLogTests(unittest.TestCase):
    def test_compilation_after_another_threads_prefix(self):
        record = "1923   22    b        JitStress::recurse (9 bytes)\r\n"
        for prefix in ("", "### Excluding compile: java.lang.Throwable::"):
            self.assertTrue(runner.compiled_method(prefix + record, "JitStress", "recurse"))

    def test_exclusions_and_markers_are_not_compilation_evidence(self):
        for text in ("### Excluding compile: JitStress::recurse\n",
                     "made not compilable on level 1 JitStress::recurse (9 bytes)\n",
                     "OPENJDK_JIT_STRESS PASS threads=4 gc=8 null=32 stack=4 roots=ok\n",
                     "1923\n22 b JitStress::recurse (9 bytes)\n",
                     "1923 22 b JitStress::field (9 bytes)\n"):
            self.assertFalse(runner.compiled_method(text, "JitStress", "recurse"))
