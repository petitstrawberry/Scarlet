import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("prepare_platform_boot", Path(__file__).with_name("prepare-boot-profile.py"))
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class PlatformBootTest(unittest.TestCase):
    def test_home_package_set_includes_the_module_metadata_provider(self):
        from desktop_inputs import PACKAGES
        self.assertIn("app/ModuleMetadata",PACKAGES["product"])

    def test_memfd_profile_overrides_the_late_stock_reset_and_rejects_drift(self):
        stock = "on post-fs-data\n    mkdir /data\n    setprop sys.use_memfd false\n    chmod 0700 /data\n"
        selected = prepare.select_memfd_in_init(stock)
        self.assertEqual(selected, stock.replace("sys.use_memfd false", "sys.use_memfd true"))
        for invalid in [stock + "    setprop sys.use_memfd false\n", selected, stock.replace("sys.use_memfd", "other")]:
            with self.subTest(source=invalid), self.assertRaises(ValueError):
                prepare.select_memfd_in_init(invalid)

    def test_retaining_stock_storage_environment_preserves_managed_classpath(self):
        stock = "on early-init\n    export ANDROID_DATA /data\n    export ASEC_MOUNTPOINT /mnt/asec\n"
        managed = "on early-init\n    export ANDROID_DATA /data\n    export BOOTCLASSPATH /system/framework/framework.jar\n"
        exports = prepare.parse_stock_environment(stock)
        result = prepare.retain_stock_environment(managed, exports)
        self.assertIn("    export ASEC_MOUNTPOINT /mnt/asec\n", result)
        self.assertEqual(result.count("    export ANDROID_DATA "), 1)
        self.assertIn("    export BOOTCLASSPATH /system/framework/framework.jar\n", result)
        self.assertEqual(prepare.retain_stock_environment(result, exports), result)
        with self.assertRaises(ValueError):
            prepare.retain_stock_environment(managed, {"ANDROID_DATA": "/other"})

    def test_unexpected_stock_environment_actions_and_substitutions_are_rejected(self):
        for line in ["on boot", "exec /system/bin/other", "export ANDROID_DATA ${other}",
                     "export ANDROID_DATA /data\nexport ANDROID_DATA /other"]:
            with self.subTest(line=line), self.assertRaises(ValueError):
                prepare.parse_stock_environment("on early-init\n" + line + "\n")
