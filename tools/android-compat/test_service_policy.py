import os
import hashlib
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from copy import deepcopy
from unittest.mock import patch

from service_policy import (ACCESS_MAGIC, CLASSES, MAGIC, PERMISSIONS, SDK_POLICY_SHA256, SDK_POLICY_VERSION,
                            AID_HEADER_SHA256, GENERIC_MAGIC, GENERIC_PARTITIONS,
                            _generic_graph, apply_boot_policy, apply_generic_boot_policy, export, export_generic,
                            generic_transition, generic_uids, validate, validate_generic, validate_metadata, verify)


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
    def seal_generic(value):
        value.pop("manifest_sha256", None)
        value["manifest_sha256"] = hashlib.sha256(json.dumps(value, sort_keys=True,
            separators=(",", ":")).encode()).hexdigest()
        return value

    def generic_manifest(self, explicit=False):
        label, target = "u:object_r:daemon_exec:s0", "u:r:daemon:s0"
        service = {"service": "daemon", "principal": 1058, "rc": "scarlet/boot/init.d/system/daemon.rc",
            "rc_sha256": "a" * 64, "disposition": "source-derived", "resolved_executable": "/system/bin/daemon",
            "file_label": label, "target": target, "executable_sha256": "b" * 64,
            "source_xattr_sha256": hashlib.sha256((label + "\0").encode()).hexdigest(),
            "policy_rules": ["allow init daemon:process transition;"], "seclabel": target if explicit else None,
            "sockets": []}
        value = {"kind": GENERIC_MAGIC, "source_sha256": SDK_POLICY_SHA256, "policy_version": SDK_POLICY_VERSION,
            "partitions": GENERIC_PARTITIONS, "aid_header_sha256": AID_HEADER_SHA256,
            "rc_files": {service["rc"]: service["rc_sha256"]}, "services": [service],
            "file_labels": {"/system/bin/daemon": label},
            "exec_transitions": [] if explicit else [["u:r:init:s0", label, target, 1058]],
            "transitions": [["u:r:init:s0", target, 1058]] if explicit else [], "socket_labels": []}
        return self.seal_generic(value)

    def test_generic_socket_delegations_follow_selected_service_options(self):
        manifest = self.generic_manifest()
        service = manifest["services"][0]
        service["sockets"] = [
            {"name": "daemon", "type": "stream", "mode": "0666", "uid": "daemon", "gid": "daemon", "seclabel": None},
            {"name": "daemon_control", "type": "seqpacket", "mode": "0660", "uid": "root", "gid": "daemon", "seclabel": None},
            {"name": "daemon_explicit", "type": "dgram", "mode": "0660", "uid": "root", "gid": "daemon",
             "seclabel": "u:object_r:daemon_socket:s0"}]
        service["socket_labels"] = ["u:object_r:daemon_socket:s0", "u:r:daemon:s0"]
        manifest["socket_labels"] = [["u:r:init:s0", "u:object_r:daemon_socket:s0"],
                                     ["u:r:init:s0", "u:r:daemon:s0"]]
        self.seal_generic(manifest)
        boot = ("scarlet-android-boot-policy-v2\nlabel 1 u:r:init:s0\n"
                "label 40 u:r:scarlet_input_bridge:s0\n")
        result = apply_generic_boot_policy(boot, manifest)
        labels = {line.split()[2]: int(line.split()[1]) for line in result.splitlines() if line.startswith("label ")}
        self.assertIn(f"socket-create 1 {labels['u:r:daemon:s0']}\n", result)
        self.assertIn(f"socket-create 1 {labels['u:object_r:daemon_socket:s0']}\n", result)
        self.assertEqual(result.count("socket-create "), 2)
        self.assertEqual(apply_generic_boot_policy(result, manifest), result)

        managed = self.generic_manifest()
        managed["services"][0].update(disposition="profile-managed", sockets=service["sockets"])
        managed["services"][0].pop("socket_labels", None)
        managed["file_labels"], managed["exec_transitions"], managed["transitions"] = {}, [], []
        self.seal_generic(managed)
        self.assertEqual(apply_generic_boot_policy(boot, managed), boot)

        changed = deepcopy(manifest)
        changed["services"][0]["sockets"].append({"name": "unmanaged", "type": "stream", "mode": "0666",
            "uid": "root", "gid": "root", "seclabel": "u:object_r:unmanaged_socket:s0"})
        self.seal_generic(changed)
        with self.assertRaisesRegex(ValueError, "differs from options"):
            apply_generic_boot_policy(boot, changed)

    def test_generic_graph_records_only_declared_service_sockets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            relative = "scarlet/boot/init.rc"
            path = root / relative
            path.parent.mkdir(parents=True)
            data = ("service logd /system/bin/logd\n"
                    "    socket logd stream 0666 logd logd\n"
                    "    socket logdw dgram+passcred 0222 logd logd\n"
                    "    user logd\n\n"
                    "service tombstoned /system/bin/tombstoned\n"
                    "    socket tombstoned_crash seqpacket 0666 system system\n\n"
                    "service daemon /system/bin/daemon\n").encode()
            path.write_bytes(data)
            metadata = {"profile": {"boot_properties": {}}, "files": {relative: hashlib.sha256(data).hexdigest()}}
            _, services = _generic_graph(root, metadata)
            records = {service["service"]: service for service in services}
            self.assertEqual([socket["name"] for socket in records["logd"]["sockets"]], ["logd", "logdw"])
            self.assertEqual([socket["name"] for socket in records["tombstoned"]["sockets"]], ["tombstoned_crash"])
            self.assertEqual(records["daemon"]["sockets"], [])

    def test_generic_relabels_keep_source_origin_kind_and_existing_ids_exact(self):
        manifest = self.generic_manifest()
        manifest["file_relabels"] = [["u:r:init:s0", "u:object_r:unlabeled:s0", "u:object_r:dalvikcache_data_file:s0", 1]]
        manifest["file_relabel_evidence"] = ["allow init dalvikcache_data_file:file relabelto;"]
        self.seal_generic(manifest)
        boot = ("scarlet-android-boot-policy-v2\nlabel 1 u:r:init:s0\n"
                "label 40 u:r:scarlet_input_bridge:s0\nlabel 100 u:object_r:unlabeled:s0\nfile-default 100\n")
        result = apply_generic_boot_policy(boot, manifest)
        labels = {line.split()[2]: int(line.split()[1]) for line in result.splitlines() if line.startswith("label ")}
        self.assertEqual(labels["u:r:scarlet_input_bridge:s0"], 40)
        target = labels["u:object_r:dalvikcache_data_file:s0"]
        self.assertIn(f"file-relabel 1 100 {target} 1\n", result)
        self.assertNotIn(f"file-relabel 1 100 {target} 2\n", result)
        self.assertEqual(apply_generic_boot_policy(result, manifest), result)
        with self.assertRaisesRegex(ValueError, "file default"):
            apply_generic_boot_policy(boot.replace("file-default 100\n", ""), manifest)
        for index, value in [(0, "u:r:untrusted_app:s0"), (1, "u:object_r:system_data_file:s0"), (2, "u:object_r:app_data_file:s0:c1"), (3, 8)]:
            invalid = deepcopy(manifest)
            invalid["file_relabels"][0][index] = value
            self.seal_generic(invalid)
            with self.subTest(index=index), self.assertRaises(ValueError):
                validate_generic(invalid)

    def test_generic_apply_preserves_runtime_ids_and_rejects_conflicting_bindings(self):
        boot = ("scarlet-android-boot-policy-v2\nlabel 1 u:r:init:s0\n"
                "label 40 u:r:scarlet_input_bridge:s0\nlabel 41 u:r:webview_zygote:s0\n"
                "label 42 u:r:shared_relro:s0:c13,c260,c512,c768\n"
                "label 100 u:object_r:unlabeled:s0\nfile-default 100\n")
        manifest = self.generic_manifest()
        result = apply_generic_boot_policy(boot, manifest)
        self.assertTrue(result.startswith(boot))
        self.assertEqual(result, apply_generic_boot_policy(result, manifest))
        self.assertIn("file-exec 1 101 102 1058\n", result)
        self.assertNotIn("service-allow", result)
        self.assertNotIn("file-create", result)
        self.assertNotIn("file-relabel", result)
        for conflict in ("file-label 100 /system/bin/daemon\n",
                         "label 101 u:object_r:daemon_exec:s0\nfile-exec 1 101 1 1058\n"):
            with self.assertRaisesRegex(ValueError, "conflict"):
                apply_generic_boot_policy(boot + conflict, manifest)
        changed = deepcopy(manifest); changed["exec_transitions"][0][3] = 0
        with self.assertRaisesRegex(ValueError, "manifest"):
            apply_generic_boot_policy(boot, changed)
        self.seal_generic(changed)
        with self.assertRaisesRegex(ValueError, "evidence"):
            apply_generic_boot_policy(boot, changed)

    def test_generic_explicit_seclabel_never_becomes_automatic_exec(self):
        manifest = self.generic_manifest(explicit=True)
        result = apply_generic_boot_policy("scarlet-android-boot-policy-v2\nlabel 1 u:r:init:s0\n", manifest)
        self.assertIn("transition 1 3 1058\n", result)
        self.assertNotIn("file-exec", result)
        self.assertEqual(result, apply_generic_boot_policy(result, manifest))
        changed = deepcopy(manifest); changed["services"][0]["seclabel"] = "u:r:other:s0"
        with self.assertRaisesRegex(ValueError, "target mismatch"):
            validate_generic(self.seal_generic(changed))

    def test_generic_uid_resolution_is_bound_to_source_not_host_accounts(self):
        header = (b"#define AID_ROOT 0\n#define AID_NOBODY 9999\n#define AID_TOMBSTONED 1058\n"
                  b"#define AID_MEDIA_CODEC 1046\n#define AID_MEDIA_EX 1040\n#define AID_MEDIA_DRM 1031\n")
        with self.assertRaisesRegex(ValueError, "header source"):
            generic_uids(b"", header)
        with patch("service_policy.AID_HEADER_SHA256", hashlib.sha256(header).hexdigest()):
            aids = generic_uids(b"vendor_test:x:2901:2901::/:/bin/false\n", header)
            self.assertEqual((aids["root"], aids["nobody"], aids["tombstoned"], aids["mediacodec"]),
                             (0, 9999, 1058, 1046))
            self.assertEqual(aids["vendor_test"], 2901)
            self.assertNotIn("petitstrawberry", aids)
            with self.assertRaisesRegex(ValueError, "conflicting"):
                generic_uids(b"tombstoned:x:0:0::/:/bin/false\n", header)

    def test_generic_transition_requires_authorization_and_rejects_conditional_rules(self):
        class ExecRule(Rule):
            default = "daemon"
            def __str__(self): return "compiled rule"
        policy = SimpleNamespace(lookup_type=lambda _: None,
            lookup_typeattr=lambda _: SimpleNamespace(expand=lambda: ["daemon"]),
            lookup_role=lambda _: SimpleNamespace(types=lambda: ["daemon"]))
        transitions, allows, constraints = [ExecRule()], [ExecRule()], []
        api = SimpleNamespace(ConstraintQuery=lambda *a, **k: SimpleNamespace(results=lambda: constraints),
            TERuleQuery=lambda *a, **k: SimpleNamespace(results=lambda:
                transitions if k["ruletype"] == ["type_transition"] else allows),
            exception=SimpleNamespace(RuleNotConditional=RuleNotConditional))
        label = "u:object_r:daemon_exec:s0"
        self.assertEqual(generic_transition(policy, api, label)[0], "u:r:daemon:s0")
        transitions.clear()
        with self.assertRaisesRegex(ValueError, "missing or ambiguous"):
            generic_transition(policy, api, label)
        self.assertEqual(generic_transition(policy, api, label, "u:r:daemon:s0")[0], "u:r:daemon:s0")
        allows.clear()
        with self.assertRaisesRegex(ValueError, "missing generic"):
            generic_transition(policy, api, label, "u:r:daemon:s0")
        allows.append(SimpleNamespace(conditional=True))
        with self.assertRaisesRegex(ValueError, "conditional"):
            generic_transition(policy, api, label, "u:r:daemon:s0")
        allows[:] = [ExecRule()]
        transitions.append(SimpleNamespace(conditional=True))
        with self.assertRaisesRegex(ValueError, "conditional"):
            generic_transition(policy, api, label)

    @unittest.skipUnless(os.getenv("SCARLET_TEST_GENERIC_ROOT"), "requires source images, assembled init graph and SETools")
    def test_generic_export_against_real_source_images_and_compiled_policy(self):
        import setools
        policy_path = Path(os.environ["SCARLET_TEST_COMPILED_POLICY"])
        policy = setools.SELinuxPolicy(str(policy_path))
        manifest = export_generic(policy, setools, policy_path, os.environ["SCARLET_TEST_GENERIC_ROOT"],
            os.environ["SCARLET_TEST_GENERIC_BOOT_PROFILE"], os.environ["SCARLET_TEST_GENERIC_PARTITIONS"],
            os.environ["SCARLET_TEST_GENERIC_PACKAGES"], os.environ["SCARLET_TEST_GENERIC_AID_HEADER"])
        records = {record["service"]: record for record in manifest["services"]}
        for name, uid in (("tombstoned", 1058), ("traced", 9999), ("traced_probes", 9999),
                          ("traced_perf", 9999), ("mm_events", 9999), ("perfetto_trace_on_boot", 2000)):
            record = records[name]
            self.assertEqual(record["principal"], uid)
            self.assertEqual(record["disposition"], "source-derived")
            target = {str(rule.default) for rule in setools.TERuleQuery(policy,
                ruletype=["type_transition"], source="init", target=record["file_label"].split(":")[2],
                tclass=["process"]).results()}
            self.assertEqual(target, {record["target"].split(":")[2]})
        self.assertEqual(records["ueventd"]["resolved_executable"], "/system/bin/init")
        self.assertFalse(any(rule[1] == "u:object_r:init_exec:s0" for rule in manifest["exec_transitions"]))
        for name in ("logd", "tombstoned", "traced", "mdnsd"):
            self.assertIn(["u:r:init:s0", records[name]["target"]], manifest["socket_labels"])
        relabels = manifest["file_relabels"]
        self.assertIn(["u:r:init:s0", "u:object_r:unlabeled:s0", "u:object_r:dalvikcache_data_file:s0", 1], relabels)
        # This preparation belongs to init; don't invent an installer grant.
        self.assertNotIn(["u:r:installd:s0", "u:object_r:unlabeled:s0", "u:object_r:system_data_file:s0", 2], relabels)
        self.assertNotIn(["u:r:init:s0", "u:object_r:unlabeled:s0", "u:object_r:app_data_file:s0", 2], relabels)
        for target in ("dalvikcache_data_file", "vold_data_file", "system_app_data_file"):
            for kind, name in ((1, "file"), (2, "dir")):
                self.assertIn(["u:r:init:s0", "u:object_r:unlabeled:s0", f"u:object_r:{target}:s0", kind], relabels)
                for label, permission in (("unlabeled", "relabelfrom"), (target, "relabelto")):
                    self.assertTrue(list(setools.TERuleQuery(policy, ruletype=["allow"], source="init",
                        target=label, tclass=[name], perms=[permission]).results()))
        from boot_profile import shield_policy
        profile = json.loads(Path("tools/android-compat/profiles/scarlet-arm64-flattened.json").read_text())
        contexts = ["* " + label for label in {label for values in profile["property_grants"].values() for label in values}]
        boot = shield_policy(profile, ["\n".join(contexts)], manifest, os.environ["SCARLET_TEST_GENERIC_ROOT"])
        labels = {line.split()[2]: int(line.split()[1]) for line in boot.splitlines() if line.startswith("label ")}
        self.assertEqual(labels["u:r:scarlet_input_bridge:s0"], 40)
        self.assertEqual(labels["u:r:webview_zygote:s0"], 41)
        self.assertEqual(labels["u:r:shared_relro:s0:c13,c260,c512,c768"], 42)
        self.assertEqual(apply_generic_boot_policy(boot, manifest), boot)

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
