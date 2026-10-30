from copy import deepcopy
from pathlib import Path
import json
import tempfile
import unittest

from art_inputs import SDK_SHA256, sha256
from boot_profile import MANIFEST, POLICY, guest_alias, shield_policy, stage_boot_inputs, validate_profile, verify_boot_inputs


def profile():
    return {"version": 1, "name": "test", "apex_modules": ["com.android.runtime"],
            "boot_properties": {"ro.apex.updatable": "false", "ro.zygote": "zygote64"},
            "bootconfig": {"androidboot.init_rc": "/scarlet/boot/init.rc"},
            "subjects": {"u:r:init:s0": 1, "u:r:vendor_init:s0": 2},
            "transitions": [["u:r:init:s0", "u:r:vendor_init:s0", 0]],
            "property_grants": {"u:r:init:s0": ["u:object_r:gsid_prop:s0"]},
            "init_imports": ["/system/etc/init/hw/init.rc"]}


class BootProfileTest(unittest.TestCase):
    def test_grants_are_explicit_and_not_inferred_from_registered_contexts(self):
        p = profile()
        contexts = ["ro.gsid. u:object_r:gsid_prop:s0\n* u:object_r:default_prop:s0\n"]
        text = shield_policy(p, contexts)
        self.assertIn("label 100 u:object_r:default_prop:s0\n", text)
        self.assertIn("label 101 u:object_r:gsid_prop:s0\n", text)
        self.assertIn("property-set 1 101\n", text)
        self.assertNotIn("property-set 1 100\n", text)
        self.assertNotIn("property-set 2 ", text)
        self.assertIn("transition 1 2 0\n", text)
        with self.assertRaisesRegex(ValueError, "absent"):
            shield_policy(p, ["* u:object_r:default_prop:s0\n"])

    def test_updatable_apex_ambiguous_subjects_and_unsafe_input_syntax_rejected(self):
        changes = [("boot_properties", {"ro.apex.updatable": "true"}),
                   ("subjects", {"u:r:init:s0": 1, "u:r:vendor_init:s0": 1}),
                   ("bootconfig", {"androidboot.init_rc": '/ok"\nextra="x'}),
                   ("apex_modules", ["com.android.runtime", "com.android.runtime"]),
                   ("init_imports", ["/../../etc/secret.rc"]),
                   ("transitions", [["u:r:init:s0", "u:r:unknown:s0", 0]])]
        for field, value in changes:
            p = deepcopy(profile()); p[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate_profile(p)
        self.assertEqual(guest_alias("system/apex/com.android.art/lib64/libc++.so", "/system/lib64/libc++.so"), "/system/lib64/libc++.so")
        for target in ["/etc/passwd", "../../../../../../etc/passwd", "/system/lib64/bad\n.so"]:
            with self.assertRaises(ValueError):
                guest_alias("system/apex/com.android.art/lib64/a.so", target)

    def test_file_label_authority_does_not_grant_property_value_writes(self):
        p=profile();p.update(version=2,file_labels={},exec_transitions=[])
        p["file_policy"]={"default":"u:object_r:unlabeled:s0","create":[],"relabel":[],
            "property_areas":[{"source":"u:r:init:s0","targets":["u:object_r:default_prop:s0"]}]}
        text=shield_policy(p,["* u:object_r:default_prop:s0\nro.gsid. u:object_r:gsid_prop:s0\n"])
        labels={line.split()[2]:line.split()[1] for line in text.splitlines() if line.startswith("label ")}
        default=labels["u:object_r:default_prop:s0"];unlabeled=labels["u:object_r:unlabeled:s0"]
        self.assertIn(f"file-relabel 1 {unlabeled} {default} 1\n",text)
        self.assertNotIn(f"property-set 1 {default}\n",text)
        self.assertNotIn("file-exec ",text)
        p["file_policy"]["property_areas"][0]["source"]="u:r:unknown:s0"
        with self.assertRaises(ValueError):validate_profile(p)

    def test_service_grants_are_explicit_and_preserve_subject_identifiers(self):
        p=profile();p.update(version=2,file_labels={},exec_transitions=[])
        p["service_grants"]=[["u:r:init:s0","u:r:vendor_init:s0",4],["u:r:init:s0","u:object_r:manager_service:s0",3]]
        text=shield_policy(p,["* u:object_r:gsid_prop:s0\n"])
        self.assertIn("label 2 u:r:vendor_init:s0\n",text)
        self.assertEqual(text.count(" u:r:vendor_init:s0\n"),1)
        self.assertIn("service-allow 1 2 4\n",text)
        self.assertNotIn("property-set 1 101\n",text)
        for bad in [["u:r:unknown:s0","u:object_r:manager_service:s0",3],["u:r:init:s0","u:r:unknown:s0",1],["u:r:init:s0","u:object_r:manager_service:s0",8]]:
            changed=deepcopy(p);changed["service_grants"]=[bad]
            with self.assertRaises(ValueError):validate_profile(changed)

    def test_staging_detects_file_changes_base_conflicts_and_alias_ancestors(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            inputs, runtime, stock, root = [base / name for name in ["inputs", "runtime", "stock", "root"]]
            for path in [inputs, runtime, stock, root]: path.mkdir()
            (runtime / "art-runtime.json").write_text("runtime identity")
            (stock / "stock-inputs.json").write_text("stock identity")
            contexts = "* u:object_r:gsid_prop:s0\n"
            (stock / "contexts").write_text(contexts)
            (inputs / POLICY).parent.mkdir(parents=True)
            (inputs / POLICY).write_text(shield_policy(profile(), [contexts]))
            data = inputs / "system/apex/com.android.runtime/bin/linker64"
            data.parent.mkdir(parents=True); data.write_bytes(b"verified payload")
            record = {"kind": "scarlet-android-boot-inputs-v1", "result": "PREPARED", "sdk_sha256": SDK_SHA256,
                      "runtime_manifest_sha256": sha256(runtime / "art-runtime.json"),
                      "stock_manifest_sha256": sha256(stock / "stock-inputs.json"), "profile": profile(),
                      "property_context_files": ["contexts"], "replacements": {}, "symlinks": {},
                      "files": {str(p.relative_to(inputs)): sha256(p) for p in inputs.rglob("*") if p.is_file()},
                      "directories": [str(p.relative_to(inputs)) for p in inputs.rglob("*") if p.is_dir()]}
            (inputs / MANIFEST).write_text(json.dumps(record))
            verify_boot_inputs(inputs, runtime, stock)
            data.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "changed"):
                stage_boot_inputs(inputs, runtime, stock, root)
            data.write_bytes(b"verified payload")
            (root / "system").symlink_to(runtime, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "alias"):
                stage_boot_inputs(inputs, runtime, stock, root)
            (root / "system").unlink()
            stage_boot_inputs(inputs, runtime, stock, root)
            target = root / data.relative_to(inputs)
            self.assertEqual(target.read_bytes(), b"verified payload")
            target.write_bytes(b"different base")
            with self.assertRaisesRegex(ValueError, "conflicts"):
                stage_boot_inputs(inputs, runtime, stock, root)


if __name__ == "__main__":
    unittest.main()
