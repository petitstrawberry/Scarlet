import os
import hashlib
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

from service_policy import (ACCESS_MAGIC, CLASSES, MAGIC, PERMISSIONS, SDK_POLICY_SHA256, SDK_POLICY_VERSION,
                            apply_boot_policy, export, validate, validate_metadata, verify)


class RuleNotConditional(Exception):
    pass


class Rule:
    source = SimpleNamespace(expand=lambda: ["app"])
    target = SimpleNamespace(expand=lambda: ["endpoint"])
    tclass = "service_manager"
    perms = {"find"}

    @property
    def conditional(self):
        raise RuleNotConditional()


class ServicePolicyTests(unittest.TestCase):
    @staticmethod
    def manifest(text, version=SDK_POLICY_VERSION):
        domains, rules = validate(text)
        return {"kind": text.splitlines()[0], "source_sha256": SDK_POLICY_SHA256,
                "policy_version": version, "domains": len(domains), "rules": len(rules),
                "text_sha256": hashlib.sha256(text.encode()).hexdigest()}

    def test_verified_manifest_exports_policy_version_without_inventing_it(self):
        boot = "scarlet-android-boot-policy-v2\nlabel 1 u:r:init:s0\nlabel 2 u:r:vendor_init:s0\n"
        services = MAGIC + "\ndomain vendor_init\nallow vendor_init u:object_r:manager_service:s0 3\n"
        metadata = self.manifest(services)
        versioned = apply_boot_policy(boot, services, metadata)
        self.assertEqual(versioned.splitlines()[:2], [boot.splitlines()[0], "policy-version 30"])
        self.assertEqual(versioned.count("policy-version 30\n"), 1)
        self.assertNotIn("policy-version", apply_boot_policy(boot, services))
        self.assertEqual(apply_boot_policy(versioned, services, metadata), versioned)

        for invalid in (True, 0, 15, 31, 0xffffffff, "30"):
            metadata["policy_version"] = invalid
            with self.subTest(version=invalid):
                with self.assertRaisesRegex(ValueError, "version"):
                    validate_metadata(metadata, services)
                with self.assertRaisesRegex(ValueError, "version"):
                    apply_boot_policy(versioned, services, metadata)

    def test_manager_manifest_binds_version_to_exact_source_and_text(self):
        services = MAGIC + "\ndomain app\nallow app u:object_r:endpoint:s0 2\n"
        metadata = self.manifest(services)
        self.assertEqual(validate_metadata(metadata, services), SDK_POLICY_VERSION)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "service-policy.txt").write_text(services)
            (root / "service-policy.json").write_text(json.dumps(metadata))
            self.assertEqual(verify(root)["policy_version"], 30)
            for field, value in (("text_sha256", "0" * 64), ("source_sha256", "0" * 64),
                                 ("domains", 2), ("rules", 2), ("policy_version", 0),
                                 ("policy_version", 31), ("policy_version", 0xffffffff)):
                changed = dict(metadata)
                changed[field] = value
                (root / "service-policy.json").write_text(json.dumps(changed))
                with self.subTest(field=field), self.assertRaises(ValueError):
                    verify(root)
            (root / "service-policy.json").write_text(json.dumps(metadata))
            (root / "service-policy.txt").write_text(services + "# changed\n")
            with self.assertRaises(ValueError):
                verify(root)

    def test_compiled_grants_apply_to_each_category_instance_of_the_same_domain(self):
        boot = "scarlet-android-boot-policy-v2\nlabel 1 u:r:init:s0\nlabel 40 u:r:shared_relro:s0\nlabel 41 u:r:shared_relro:s0:c13,c260,c512,c768\nlabel 42 u:r:shared_relro:s0:c14,c260,c512,c768\nlabel 100 u:object_r:activity_service:s0\nservice-allow 41 100 7\nservice-allow 1 100 4\n"
        services = MAGIC + "\ndomain shared_relro\nallow shared_relro u:object_r:activity_service:s0 2\n"
        result = apply_boot_policy(boot, services)
        for identifier in (40, 41, 42):
            self.assertIn(f"service-allow {identifier} 100 2\n", result)
            self.assertNotIn(f"service-allow {identifier} 100 7\n", result)
        self.assertIn("service-allow 1 100 4\n", result)
        self.assertEqual(apply_boot_policy(result, services), result)

    def test_boot_subject_rules_are_replaced_without_changing_other_classes(self):
        boot = "scarlet-android-boot-policy-v2\nlabel 1 u:r:init:s0\nlabel 10 u:r:system_server:s0\nlabel 100 u:object_r:wifi_service:s0\nservice-allow 10 100 7\nservice-allow 1 100 4\nproperty-set 10 100\n"
        services = MAGIC + "\ndomain system_server\nallow system_server u:object_r:wifi_service:s0 3\nallow system_server u:object_r:wifinl80211_service:s0 2\n"
        result = apply_boot_policy(boot, services)
        self.assertNotIn("service-allow 10 100 7", result)
        self.assertIn("service-allow 10 100 3", result)
        self.assertIn("service-allow 1 100 4", result)
        self.assertIn("property-set 10 100", result)
        self.assertIn("label 101 u:object_r:wifinl80211_service:s0", result)
        self.assertIn("service-allow 10 101 2", result)
        self.assertEqual(apply_boot_policy(result, services), result)

    def test_property_rules_replace_only_selected_subjects_and_preserve_classes(self):
        boot = "scarlet-android-boot-policy-v2\nlabel 1 u:r:init:s0\nlabel 2 u:r:system_server:s0\nlabel 3 u:object_r:device_owner_prop:s0\nproperty-set 1 3\nproperty-set 2 3\nsocket-create 2 3\n"
        text = ACCESS_MAGIC + "\ndomain system_server\nallow system_server u:object_r:device_owner_prop:s0 2 1\n"
        result = apply_boot_policy(boot, text)
        self.assertEqual(result.count("property-set 2 3"), 1)
        self.assertIn("property-set 1 3", result)
        self.assertIn("socket-create 2 3", result)
        for invalid in ("2 2", "2 7", "3 1", "1"):
            with self.assertRaises(ValueError):
                validate(ACCESS_MAGIC + "\ndomain system_server\nallow system_server u:object_r:device_owner_prop:s0 " + invalid + "\n")

    def test_export_rejects_unrepresentable_constraints_and_class_overlap(self):
        class Kind:
            perms = set(PERMISSIONS)

            def __init__(self, name): self.name = name
            def __str__(self): return self.name

        policy = SimpleNamespace(
            classes=lambda: [Kind(c) for c in CLASSES],
            lookup_typeattr=lambda _: SimpleNamespace(expand=lambda: ["app"]))
        rules, constraints = [Rule()], []
        api = SimpleNamespace(
            ConstraintQuery=lambda *a, **k: SimpleNamespace(results=lambda: constraints),
            TERuleQuery=lambda *a, **k: SimpleNamespace(results=lambda: rules),
            exception=SimpleNamespace(RuleNotConditional=RuleNotConditional))
        self.assertIn("allow app u:object_r:endpoint:s0 2\n", export(policy, api))
        constraints.append(object())
        with self.assertRaisesRegex(ValueError, "constraints"): export(policy, api)
        constraints.clear()
        other = Rule(); other.tclass = "hwservice_manager"; rules.append(other)
        with self.assertRaisesRegex(ValueError, "overlap"): export(policy, api)
        rules.clear(); rules.append(SimpleNamespace(conditional=True))
        with self.assertRaisesRegex(ValueError, "conditional"): export(policy, api)

    def test_manifest_syntax_cannot_expand_permissions_or_unknown_domains(self):
        text = MAGIC + "\ndomain app\n"
        for row in ["allow app u:object_r:endpoint:s0 8", "allow other u:object_r:endpoint:s0 2",
                    "allow app u:object_r:endpoint:s0:c1 2", "domain app"]:
            with self.assertRaises(ValueError): validate(text + row + "\n")
        duplicate = "allow app u:object_r:endpoint:s0 2\n"
        with self.assertRaises(ValueError): validate(text + duplicate * 2)

    @unittest.skipUnless(os.getenv("SCARLET_TEST_COMPILED_POLICY"), "requires pinned Android policy and SETools")
    def test_export_matches_independent_compiled_policy_queries(self):
        import setools
        policy = setools.SELinuxPolicy(os.environ["SCARLET_TEST_COMPILED_POLICY"])
        _, exported = validate(export(policy, setools))
        for source in ("mediaprovider_app", "untrusted_app", "isolated_app", "priv_app"):
            for target, kind in (("mediaserver_service", "service_manager"),
                                 ("hidl_manager_hwservice", "hwservice_manager"),
                                 ("hal_omx_hwservice", "hwservice_manager"),
                                 ("keystore_maintenance_service", "service_manager")):
                for permission, bit in (("add", 1), ("find", 2)):
                    with self.subTest(source=source, target=target, permission=permission):
                        expected = bool(list(setools.TERuleQuery(policy, ruletype=["allow"],
                            source=source, target=target, tclass=[kind], perms=[permission]).results()))
                        actual = bool(exported.get((source, f"u:object_r:{target}:s0", 1), 0) & bit)
                        self.assertEqual(expected, actual)
        _, exported = validate(export(policy, setools, ["system_server"], properties=True))
        for source in ("system_server", "bluetooth", "untrusted_app", "mediaprovider_app"):
            for target in ("device_logging_prop", "binder_cache_bluetooth_server_prop", "bluetooth_prop", "default_prop"):
                expected = bool(list(setools.TERuleQuery(policy, ruletype=["allow"], source=source,
                    target=target, tclass=["property_service"], perms=["set"]).results()))
                self.assertEqual(bool(exported.get((source, f"u:object_r:{target}:s0", 2), 0) & 1), expected)


if __name__ == "__main__":
    unittest.main()
