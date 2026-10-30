from copy import deepcopy
import unittest

from host_services import (BINARY, CONTRACT, MANAGED_RC, STOCK_TEXT, build_suspend,
                           HW_MANAGER,HW_BINARY,HW_MANAGED_RC,build_hwmanager,
                           imports, validate_selection, validate_staged)


class HostServicesTest(unittest.TestCase):
    def test_hwmanager_preserves_the_verified_stock_declaration(self):
        rc=b"service hwservicemanager /system/bin/hwservicemanager\n    user system\n    disabled\n"
        managed,proof=build_hwmanager(rc,b"verified hwmanager")
        self.assertEqual(managed,rc)
        profile={"host_services":[HW_MANAGER]}
        self.assertEqual(imports(profile),["/"+HW_MANAGED_RC])
        metadata={"profile":profile,"host_service_inputs":{HW_MANAGER:proof},
                  "files":{HW_BINARY:proof["binary_sha256"],HW_MANAGED_RC:proof["managed_rc_sha256"]}}
        validate_staged(metadata)
        metadata["files"][HW_MANAGED_RC]="changed"
        with self.assertRaises(ValueError):validate_staged(metadata)
        with self.assertRaises(ValueError):build_hwmanager(b"service wrong /bin/wrong\n",b"binary")
    def test_suspend_requires_provider_and_matching_contract(self):
        profile = {"host_services": ["system_suspend"],
                   "vintf": {"framework": [CONTRACT], "device": []}}
        self.assertEqual(imports(profile), ["/" + MANAGED_RC])
        for key in ("host_services", "vintf"):
            invalid = deepcopy(profile)
            invalid.pop(key)
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_selection(invalid)
        with self.assertRaises(ValueError):
            validate_selection({"host_services": ["unknown"]})

    def test_stock_rc_is_checked_and_unsupported_capability_is_removed(self):
        managed, proof = build_suspend(STOCK_TEXT.encode(), b"verified binary")
        self.assertIn(b"class early_hal", managed)
        self.assertIn(b"user system", managed)
        self.assertNotIn(b"capabilities", managed)
        self.assertFalse(proof["host_suspend"])
        with self.assertRaises(ValueError):
            build_suspend(STOCK_TEXT.replace("user system", "user root").encode(), b"binary")

    def test_staging_checks_provider_and_generated_configuration(self):
        _, proof = build_suspend(STOCK_TEXT.encode(), b"binary")
        metadata = {"profile": {"host_services": ["system_suspend"],
                               "vintf": {"framework": [CONTRACT], "device": []}},
                    "host_service_inputs": {"system_suspend": proof},
                    "files": {BINARY: proof["binary_sha256"], MANAGED_RC: proof["managed_rc_sha256"]}}
        validate_staged(metadata)
        for field in (BINARY, MANAGED_RC):
            invalid = deepcopy(metadata)
            invalid["files"][field] = "changed"
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate_staged(invalid)
        metadata.pop("host_service_inputs")
        with self.assertRaises(ValueError):
            validate_staged(metadata)
