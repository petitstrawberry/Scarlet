import os
from pathlib import Path
from types import SimpleNamespace
import unittest

from service_policy import CLASSES, MAGIC, PERMISSIONS, apply_boot_policy, export, validate


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
                        actual = bool(exported.get((source, f"u:object_r:{target}:s0"), 0) & bit)
                        self.assertEqual(expected, actual)


if __name__ == "__main__":
    unittest.main()
