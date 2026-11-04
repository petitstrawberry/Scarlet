"""Round-trip real ext2 images through unprivileged host staging."""
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from seed_image import (
    _list_extended_attributes,
    _read_extended_attributes,
    inode_metadata,
    restore_inode_metadata,
    refresh_native_files,
    stage_seed_data,
)


class SeedDirectoryEntriesTest(unittest.TestCase):
    def test_warm_stage_preserves_deletions_and_symbolic_links(self):
        with tempfile.TemporaryDirectory() as directory:
            source=Path(directory)/"source";target=Path(directory)/"staging/data"
            source.mkdir();target.mkdir(parents=True)
            (source/"packages.xml").write_text("new inventory")
            (source/"link").symlink_to("packages.xml")
            (target/"packages.xml").write_text("old inventory")
            (target/"packages-backup.xml").write_text("deleted backup")
            stage_seed_data(source,target)
            self.assertEqual((target/"packages.xml").read_text(),"new inventory")
            self.assertFalse((target/"packages-backup.xml").exists())
            self.assertTrue((target/"link").is_symlink())
            self.assertEqual((source/"packages.xml").read_text(),"new inventory")
            self.assertEqual(sorted(p.name for p in target.parent.iterdir()),["data"])

    def test_failed_seed_publication_restores_the_previous_generated_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            source=Path(directory)/"source";target=Path(directory)/"staging/data"
            source.mkdir();target.mkdir(parents=True)
            (source/"new").write_text("new");(target/"old").write_text("old")
            rename=Path.rename
            def fail_incoming(path,destination):
                if path.name=="incoming":raise OSError("publication failed")
                return rename(path,destination)
            with patch.object(Path,"rename",fail_incoming),self.assertRaises(OSError):
                stage_seed_data(source,target)
            self.assertEqual((target/"old").read_text(),"old")
            self.assertFalse((target/"new").exists())
            self.assertEqual(sorted(p.name for p in target.parent.iterdir()),["data"])

    def test_unused_empty_entries_do_not_hide_live_metadata(self):
        listing = subprocess.CompletedProcess([], 0, stdout="/11/040755/1000/1000/.//\n/0/000000/0/0//0/\n/12/100640/10027/10027/preferences/12/\n")
        with patch("seed_image.subprocess.run", return_value=listing), \
                patch("seed_image._read_extended_attributes", return_value={
                    "/data": {}, "/data/preferences": {}}):
            metadata = inode_metadata(Path("image.ext2"), "/data")
        self.assertEqual(set(metadata), {"", "preferences"})
        self.assertEqual(metadata["preferences"], {"mode": 0o100640, "uid": 10027, "gid": 10027})

    def test_live_entries_still_require_a_name(self):
        listing = subprocess.CompletedProcess([], 0, stdout="/12/100640/1000/1000//12/\n")
        with patch("seed_image.subprocess.run", return_value=listing):
            with self.assertRaisesRegex(ValueError, "empty live seed inode name"):
                inode_metadata(Path("image.ext2"), "/data")

    def test_corrupt_ea_listing_is_rejected(self):
        listing = subprocess.CompletedProcess([], 0, stdout=(
            'debugfs: ea_list "/data/file"\n'
            "Extended attributes:\n"
            "  user.truncated (8) = 61 62\n"
            "  malformed\n"), stderr="")
        with tempfile.TemporaryDirectory() as temporary, \
                patch("seed_image._run_script", return_value=listing):
            with self.assertRaisesRegex(ValueError, "unrecognized debugfs EA entry"):
                _list_extended_attributes(Path("image.ext2"), ["/data/file"], Path(temporary))

    def test_truncated_ea_value_is_rejected(self):
        def fake_script(_commands, _image, temporary, _name, writable=False):
            self.assertFalse(writable)
            (temporary / "ea-0").write_bytes(b"short")
            return subprocess.CompletedProcess([], 0, stdout="", stderr="")

        with tempfile.TemporaryDirectory() as temporary, \
                patch("seed_image._list_extended_attributes", return_value={
                    "/data/file": {"user.binary": 9}}), \
                patch("seed_image._run_script", side_effect=fake_script):
            with self.assertRaisesRegex(ValueError, "truncated debugfs EA"):
                _read_extended_attributes(Path("image.ext2"), ["/data/file"], Path(temporary))

    def test_invalid_hex_ea_value_is_rejected_before_image_access(self):
        metadata = {"": {
            "mode": 0o100644,
            "uid": 1000,
            "gid": 1000,
            "xattrs": {"trusted.scarlet.shield.0123456789abcdef0123456789abcdef": "0g"},
        }}
        with patch("seed_image._run_script") as run:
            with self.assertRaisesRegex(ValueError, "not valid strict hex"):
                restore_inode_metadata(Path("image.ext2"), "/data", metadata)
        run.assert_not_called()


@unittest.skipUnless(shutil.which("debugfs") and shutil.which("mkfs.ext2"), "e2fsprogs required")
class SeedImageTest(unittest.TestCase):
    def test_native_refresh_preserves_data_inodes_and_guest_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            tree, staging = base / "tree", base / "staging"
            (tree / "bin").mkdir(parents=True)
            (tree / "data/app").mkdir(parents=True)
            (tree / "data/app/state").write_bytes(b"persistent state")
            (staging / "bin").mkdir(parents=True)
            for name in ("init", "bin/scarlet-ld", "bin/sws", "bin/android-runtime"):
                (tree / name).write_bytes(b"old executable")
                (staging / name).write_bytes(("new " + name).encode())
            image = base / "warm.ext2"
            with image.open("wb") as stream:
                stream.truncate(16 * 1024**2)
            subprocess.run(["mkfs.ext2", "-q", "-F", "-d", str(tree), str(image)], check=True)
            subprocess.run(["debugfs", "-w", "-R", "set_inode_field /bin/sws mode 0100710", str(image)],
                           check=True, capture_output=True)
            def inode(path):
                result = subprocess.run(["debugfs", "-R", "stat " + path, str(image)],
                                        check=True, capture_output=True, text=True)
                return result.stdout.splitlines()[0].split()[1]
            before_data = inode_metadata(image, "/data")
            identities = {path: inode(path) for path in ("/data", "/data/app", "/data/app/state")}
            records = refresh_native_files(image, staging)
            self.assertEqual(len(records), 4)
            self.assertEqual(inode_metadata(image, "/data"), before_data)
            self.assertEqual({path: inode(path) for path in identities}, identities)
            self.assertEqual(inode_metadata(image, "/bin")["sws"]["mode"], 0o100710)
            for name in ("init", "bin/scarlet-ld", "bin/sws", "bin/android-runtime"):
                result = subprocess.run(["debugfs", "-R", "cat /" + name, str(image)],
                                        check=True, capture_output=True)
                self.assertEqual(result.stdout, (staging / name).read_bytes())
            installed_inodes = {record["path"]: inode(record["path"]) for record in records}
            self.assertEqual(refresh_native_files(image, staging), records)
            self.assertEqual({path: inode(path) for path in installed_inodes}, installed_inodes)

    def test_android_identity_survives_host_staging(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            tree = base / "tree"
            app = tree / "data/user_de/0/example"
            app.mkdir(parents=True)
            (app / "preferences").write_bytes(b"persisted application state")
            (app / "alias").symlink_to("preferences")
            (app / "old-socket").touch()
            xattr_value = base / "xattr-value"
            binary_value = bytes(range(256))
            xattr_value.write_bytes(binary_value)

            def image(name, directory):
                path = base / name
                with path.open("wb") as stream:
                    stream.truncate(16 * 1024**2)
                subprocess.run(["mkfs.ext2", "-q", "-F", "-d", str(directory), str(path)], check=True)
                return path

            source = image("source.ext2", tree)
            subprocess.run(["debugfs", "-w", "-R",
                            "set_inode_field /data/user_de/0/example/old-socket mode 0140777", str(source)],
                           check=True, capture_output=True)
            subprocess.run([
                "debugfs", "-w", "-R",
                f'ea_set -f "{xattr_value}" /data/user_de/0/example/preferences '
                'trusted.scarlet.shield.0123456789abcdef0123456789abcdef',
                str(source)], check=True, capture_output=True)
            # ext2's compact ACL layout differs from the Linux xattr layout.
            # Metadata staging must retain the on-disk bytes without conversion.
            acl_value = bytes.fromhex("01000000010007000400050020000100")
            acl_file = base / "acl-value"
            acl_file.write_bytes(acl_value)
            subprocess.run([
                "debugfs", "-w", "-R",
                f'ea_set -r -f "{acl_file}" /data/user_de/0/example/preferences '
                'system.posix_acl_access', str(source)], check=True, capture_output=True)
            identities = inode_metadata(source, "/data")
            self.assertNotIn("user_de/0/example/old-socket", identities)
            self.assertEqual(
                identities["user_de/0/example/preferences"]["xattrs"],
                {"trusted.scarlet.shield.0123456789abcdef0123456789abcdef": binary_value.hex(),
                 "system.posix_acl_access": acl_value.hex()})
            serialized = json.dumps(identities, sort_keys=True)
            identities = json.loads(serialized)
            self.assertEqual(json.dumps(identities, sort_keys=True), serialized)
            for relative, fields in identities.items():
                fields["uid"] = fields["gid"] = 10027 if "example" in relative else 1000
            identities["user_de/0/example"]["mode"] = 0o42770
            identities["user_de/0/example/preferences"]["mode"] = 0o100640
            restore_inode_metadata(source, "/data", identities)

            dumped = base / "dumped"
            dumped.mkdir()
            subprocess.run(["debugfs", "-R", f"rdump /data {dumped}", str(source)],
                           check=True, capture_output=True)
            destination = image("destination.ext2", dumped)
            stale_value = base / "stale-xattr-value"
            stale_value.write_bytes(b"stale")
            subprocess.run([
                "debugfs", "-w", "-R",
                f'ea_set -f "{stale_value}" /data/user_de/0/example/preferences user.stale',
                str(destination)], check=True, capture_output=True)
            subprocess.run([
                "debugfs", "-w", "-R",
                f'ea_set -f "{stale_value}" /data/user_de/0/example user.stale-directory',
                str(destination)], check=True, capture_output=True)
            staged = inode_metadata(destination, "/data")
            self.assertNotEqual(staged, identities)

            legacy = {path: {key: value for key, value in fields.items() if key != "xattrs"}
                      for path, fields in identities.items()}
            restore_inode_metadata(destination, "/data", legacy)
            self.assertEqual(
                inode_metadata(destination, "/data")
                ["user_de/0/example/preferences"]["xattrs"],
                staged["user_de/0/example/preferences"]["xattrs"])

            clearing = json.loads(json.dumps(identities))
            clearing["user_de/0/example"]["xattrs"] = {}
            restore_inode_metadata(destination, "/data", clearing)
            self.assertEqual(inode_metadata(destination, "/data"), identities)
            content = subprocess.run(["debugfs", "-R", "cat /data/user_de/0/example/preferences",
                                      str(destination)], check=True, capture_output=True).stdout
            self.assertEqual(content, b"persisted application state")
            self.assertEqual((dumped / "data/user_de/0/example/alias").readlink(), Path("preferences"))
