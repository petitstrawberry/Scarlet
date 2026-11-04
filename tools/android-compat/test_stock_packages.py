import hashlib
import json
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest

from art_inputs import SDK_SHA256, sha256
from aosp_inventory import Image
from stock_art import PARTITIONS
from stock_packages import (GENERIC_TREES, INIT_SLOTS, MANAGER_INIT_IMPORTS, MANIFEST,
                            assemble_generic_init, authenticated_package_overrides,
                            install_image, package_trees, stage,
                            seed_objects, source_entries, validate_host_service_proofs, verify)


@unittest.skipUnless(shutil.which("debugfs") and shutil.which("mkfs.ext2"), "requires e2fsprogs")
class StockPackageImageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="stock-package-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.inputs = self.root / "inputs"
        (self.inputs / "objects").mkdir(parents=True)
        nodes = {"system/app": {"mode": stat.S_IFDIR | 0o755, "uid": 0, "gid": 2000, "size": 0}}
        for name, contents in [("Case.apk", b"UPPER CASE"), ("case.apk", b"lower case")]:
            digest = hashlib.sha256(contents).hexdigest()
            (self.inputs / "objects" / digest).write_bytes(contents)
            nodes["system/app/" + name] = {"mode": stat.S_IFREG | 0o640, "uid": 1234,
                                           "gid": 5678, "size": len(contents), "sha256": digest}
        nodes["system/app/alias.apk"] = {"mode": stat.S_IFLNK | 0o777, "uid": 0, "gid": 0,
                                          "size": 8, "target": "Case.apk"}
        metadata = {"kind": "scarlet-stock-packages-v1", "sdk_sha256": SDK_SHA256,
                    "result": "PREPARED", "partitions": dict.fromkeys(PARTITIONS, "0" * 64),
                    "trees": ["system/app"], "nodes": nodes}
        (self.inputs / MANIFEST).write_text(json.dumps(metadata))
        self.staging = self.root / "staging"
        (self.staging / "systems/android-aarch64").mkdir(parents=True)
        self.disk = self.root / "rootfs.ext2"

    def make_disk(self):
        with self.disk.open("wb") as disk: disk.truncate(32 * 1024**2)
        subprocess.run(["mkfs.ext2", "-q", "-F", "-d", str(self.staging), str(self.disk)], check=True)

    def test_case_distinct_payloads_and_guest_inode_identities_survive(self):
        stage(self.inputs, self.staging / "systems/android-aarch64")
        self.make_disk()
        proof = install_image(self.inputs, self.disk, "/systems/android-aarch64")
        self.assertEqual(proof["nodes"], 4)
        image = Image(self.disk, "debugfs")
        base = "/systems/android-aarch64/system/app/"
        self.assertEqual(image.command("cat " + base + "Case.apk"), b"UPPER CASE")
        self.assertEqual(image.command("cat " + base + "case.apk"), b"lower case")
        self.assertIn(b'Fast link dest: "Case.apk"', image.command("stat " + base + "alias.apk"))
        subprocess.run(["e2fsck", "-fn", str(self.disk)], check=True, capture_output=True)

    def test_conflicting_existing_bytes_leave_the_disk_untouched(self):
        app = self.staging / "systems/android-aarch64/system/app"
        app.mkdir(parents=True)
        (app / "Case.apk").write_bytes(b"local modified APK")
        with self.assertRaisesRegex(ValueError, "content conflict"):
            stage(self.inputs, self.staging / "systems/android-aarch64")
        self.make_disk()
        before = sha256(self.disk)
        with self.assertRaisesRegex(ValueError, "content conflict"):
            install_image(self.inputs, self.disk, "/systems/android-aarch64")
        self.assertEqual(before, sha256(self.disk))

    def test_changed_object_is_rejected_before_staging(self):
        next((self.inputs / "objects").iterdir()).write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "changed stock package"):
            verify(self.inputs)


class StockPackageProofTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="stock-package-proof-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.counter = 0

    def package(self):
        self.counter += 1
        inputs = self.root / f"inputs-{self.counter}"
        (inputs / "objects").mkdir(parents=True)
        nodes = {}

        def add(name, data):
            digest = hashlib.sha256(data).hexdigest()
            (inputs / "objects" / digest).write_bytes(data)
            nodes[name] = {"mode": stat.S_IFREG | 0o755, "uid": 0, "gid": 0,
                           "size": len(data), "sha256": digest}

        add("system/bin/daemon", b"daemon")
        add("system/bin/tombstoned", b"tombstoned")
        add("system/bin/perfetto", b"perfetto")
        add("system/bin/health", b"health")
        add("system/etc/init/daemon.rc",
            b"service daemon /system/bin/daemon\n    class core\n    capabilities KILL\n")
        add("system/etc/init/tombstoned.rc",
            b"service tombstoned /system/bin/tombstoned\n    class core\n")
        add("system/etc/init/perfetto.rc",
            b"service traced /system/bin/perfetto\n    class main\n")
        add("system/etc/init/hw/init.rc",
            b"import /system/etc/init/hw/init.${ro.zygote}.rc\n"
            b"service init_helper /system/bin/daemon\n    class core\n")
        add("system/etc/init/hw/init.zygote64.rc",
            b"service zygote /system/bin/daemon\n    class main\n    updatable\n")
        add("system_ext/etc/init/init.system_ext.rc", b"on init\n    setprop sys.test 1\n")
        metadata = {"kind": "scarlet-stock-packages-v1", "sdk_sha256": SDK_SHA256,
                    "result": "PREPARED", "partitions": dict.fromkeys(PARTITIONS, "0" * 64),
                    "trees": ["system/bin", "system/etc/init", "system_ext/etc/init"],
                    "nodes": nodes}
        (inputs / MANIFEST).write_text(json.dumps(metadata))
        return inputs, nodes

    def add_manager(self, root, metadata, name, text):
        path = root / "scarlet/boot/services" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        relative = path.relative_to(root).as_posix()
        digest = sha256(path)
        metadata["files"][relative] = digest
        metadata["host_service_inputs"][name.removesuffix(".rc")] = {
            "managed_rc_sha256": digest}

    def assembly_inputs(self):
        inputs, nodes = self.package()
        root = self.root / f"staging-{self.counter}"
        service_dir = root / "scarlet/boot/services"
        service_dir.mkdir(parents=True)
        init = "".join("import " + path + "\n" for path in MANAGER_INIT_IMPORTS)
        (root / "scarlet/boot/init.rc").write_text(init)
        metadata = {"profile": {"boot_properties": {"ro.zygote": "zygote64",
                                                     "ro.hardware": "scarlet"}},
                    "files": {"scarlet/boot/init.rc": sha256(root / "scarlet/boot/init.rc")},
                    "host_service_inputs": {}}
        self.add_manager(root, metadata, "daemon.rc",
                         "service daemon /system/bin/daemon\n    class core\n")
        self.add_manager(root, metadata, "health.rc",
                         "service health-hal /system/bin/health\n    class hal\n")
        return inputs, root, metadata, nodes

    def test_complete_init_slots_replace_stock_once_and_preserve_payloads(self):
        inputs, root, metadata, nodes = self.assembly_inputs()
        before = {name: (record["sha256"], (inputs / "objects" / record["sha256"]).read_bytes())
                  for name, record in nodes.items()}
        proof = assemble_generic_init(inputs, root, metadata)
        system_slots = sorted(path.name for path in (root / INIT_SLOTS["system"]).glob("*.rc"))
        self.assertEqual(system_slots, ["daemon.rc", "perfetto.rc", "scarlet-health.rc", "tombstoned.rc"])
        self.assertIn("tombstoned", (root / INIT_SLOTS["system"] / "tombstoned.rc").read_text())
        self.assertIn("/system/bin/perfetto", (root / INIT_SLOTS["system"] / "perfetto.rc").read_text())
        self.assertNotIn("capabilities KILL", (root / INIT_SLOTS["system"] / "daemon.rc").read_text())
        self.assertEqual(proof["replacements"][0]["source"], "system/etc/init/daemon.rc")
        self.assertEqual([item["managed"] for item in proof["additions"]],
                         ["scarlet/boot/services/health.rc"])
        self.assertEqual(proof["stock_slots"], 4)
        for name, (digest, data) in before.items():
            self.assertEqual(nodes[name]["sha256"], digest)
            self.assertEqual((inputs / "objects" / digest).read_bytes(), data)

    def test_duplicate_service_and_missing_executable_are_rejected(self):
        inputs, root, metadata, _nodes = self.assembly_inputs()
        self.add_manager(root, metadata, "duplicate.rc",
                         "service tombstoned /system/bin/tombstoned\n"
                         "service tombstoned /system/bin/tombstoned\n")
        with self.assertRaisesRegex(ValueError, "duplicate init service tombstoned"):
            assemble_generic_init(inputs, root, metadata)

        inputs, root, metadata, nodes = self.assembly_inputs()
        daemon = nodes["system/etc/init/perfetto.rc"]
        bad = b"service missing /system/bin/not-present\n    class main\n"
        digest = hashlib.sha256(bad).hexdigest()
        (inputs / "objects" / digest).write_bytes(bad)
        old_digest = nodes["system/etc/init/perfetto.rc"]["sha256"]
        nodes["system/etc/init/perfetto.rc"] = {**daemon, "size": len(bad), "sha256": digest}
        (inputs / "objects" / old_digest).unlink()
        manifest = json.loads((inputs / MANIFEST).read_text())
        manifest["nodes"] = nodes
        (inputs / MANIFEST).write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "missing init executable for missing"):
            assemble_generic_init(inputs, root, metadata)

    def test_apex_override_is_explicit_and_apex_duplicate_is_rejected(self):
        inputs, root, metadata, _nodes = self.assembly_inputs()
        binary = root / "system/apex/com.android.adbd/bin/adbd"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"adbd")
        rc = root / "system/apex/com.android.adbd/etc/init.rc"
        rc.parent.mkdir(parents=True)
        rc.write_text("service zygote /apex/com.android.adbd/bin/adbd\n"
                      "    class main\n    override\n")
        assemble_generic_init(inputs, root, metadata)
        rc.write_text("service zygote /apex/com.android.adbd/bin/adbd\n    class main\n")
        with self.assertRaisesRegex(ValueError, "duplicate init service zygote"):
            assemble_generic_init(inputs, root, metadata)

    def test_unknown_service_reference_is_not_accepted_by_default(self):
        inputs, root, metadata, nodes = self.assembly_inputs()
        old = nodes["system/etc/init/perfetto.rc"]
        changed = b"on init\n    start undeclared_provider\n"
        digest = hashlib.sha256(changed).hexdigest()
        (inputs / "objects" / digest).write_bytes(changed)
        (inputs / "objects" / old["sha256"]).unlink()
        nodes["system/etc/init/perfetto.rc"] = {**old, "size": len(changed), "sha256": digest}
        manifest = json.loads((inputs / MANIFEST).read_text())
        manifest["nodes"] = nodes
        (inputs / MANIFEST).write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "unknown init service reference undeclared_provider"):
            assemble_generic_init(inputs, root, metadata)

    def test_unauthenticated_override_and_arbitrary_files_digest_are_rejected(self):
        inputs, _nodes = self.package()
        root = self.root / "stage"
        target = root / "system/etc/init/daemon.rc"
        target.parent.mkdir(parents=True)
        target.write_text("changed")
        with self.assertRaisesRegex(ValueError, "content conflict"):
            stage(inputs, root)
        record = {"sha256": sha256(target),
                  "proofs": ["host_service_inputs.netd.files"]}
        stage(inputs, root, {"system/etc/init/daemon.rc": record})
        with self.assertRaisesRegex(ValueError, "unrecognized package override proof"):
            stage(inputs, root, {"system/etc/init/daemon.rc": {
                "sha256": sha256(target), "proofs": ["files"]}})
        self.assertEqual(authenticated_package_overrides(
            {"files": {"system/etc/init/daemon.rc": sha256(target)}}), {})

    def test_stale_netd_proof_reports_actual_fields(self):
        from host_services import NETD, NETD_BINARY, NETD_MANAGED_RC, build_netd
        stock = (b"service netd /system/bin/netd\n    class main\n"
                 b"    capabilities CHOWN DAC_OVERRIDE DAC_READ_SEARCH FOWNER IPC_LOCK KILL NET_ADMIN NET_BIND_SERVICE NET_RAW SETUID SETGID\n"
                 b"    group root net_admin\n    socket dnsproxyd stream 0660 root inet\n"
                 b"    onrestart restart zygote\n    onrestart restart zygote_secondary\n")
        _managed, proof = build_netd(stock, b"stock netd")
        proof.update(files={NETD_BINARY: proof["binary_sha256"],
                            "system/lib64/libnetd_resolv.so": "dependency"},
                     native_needed={NETD_BINARY: ["libnetd_resolv.so"],
                                    "system/lib64/libnetd_resolv.so": []})
        metadata = {"profile": {"host_services": [NETD]}, "host_service_inputs": {NETD: proof},
                    "files": {**proof["files"], NETD_MANAGED_RC: proof["managed_rc_sha256"]}}
        validate_host_service_proofs(metadata)
        metadata["host_service_inputs"][NETD]["firewall_implementation"] = "scarlet-compat-v1"
        with self.assertRaisesRegex(ValueError, "firewall_implementation='scarlet-compat-v1'"):
            validate_host_service_proofs(metadata)

    def test_generic_tree_scope_is_system_and_system_ext_only(self):
        self.assertEqual(GENERIC_TREES, {"system": ("bin", "etc/init"),
                                         "system_ext": ("bin", "etc/init")})
        self.assertIn("bin", package_trees("system"))
        self.assertIn("etc/init", package_trees("system_ext"))
        self.assertNotIn("bin", package_trees("vendor"))
        self.assertNotIn("etc/init", package_trees("vendor"))

    def test_verified_object_store_reuse_is_hardlinked(self):
        inputs, _nodes = self.package()
        output = self.root / "reused"
        output.mkdir()
        seed_objects(inputs, output)
        for source in (inputs / "objects").iterdir():
            destination = output / "objects" / source.name
            self.assertEqual(source.stat().st_ino, destination.stat().st_ino)
            self.assertEqual(source.read_bytes(), destination.read_bytes())

    def test_stock_bracket_alias_is_safe_and_path_traversal_stays_rejected(self):
        class Listing:
            def command(self, _command):
                return b"/1/120755/0/0/[/0/\n/2/100755/0/0/[[/0/\n/3/100755/0/0/toybox/7/\n"

        entries = dict(source_entries(Listing(), "/unused"))
        self.assertEqual(entries["["]["mode"], stat.S_IFLNK | 0o755)
        self.assertEqual(entries["[["]["mode"], stat.S_IFREG | 0o755)

        inputs, nodes = self.package()
        nodes["system/bin/["] = {"mode": stat.S_IFLNK | 0o777, "uid": 0, "gid": 0,
                               "size": 6, "target": "toybox"}
        manifest = json.loads((inputs / MANIFEST).read_text())
        manifest["nodes"] = nodes
        (inputs / MANIFEST).write_text(json.dumps(manifest))
        verify(inputs)

        nodes["system/bin/../escape"] = {"mode": stat.S_IFREG | 0o644,
                                          "uid": 0, "gid": 0, "size": 0,
                                          "sha256": "0" * 64}
        manifest["nodes"] = nodes
        (inputs / MANIFEST).write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "invalid stock package path"):
            verify(inputs)


if __name__ == "__main__":
    unittest.main()
