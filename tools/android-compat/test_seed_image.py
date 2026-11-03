"""Round-trip real ext2 images through unprivileged host staging."""
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from seed_image import inode_metadata, restore_inode_metadata


@unittest.skipUnless(shutil.which("debugfs") and shutil.which("mkfs.ext2"), "e2fsprogs required")
class SeedImageTest(unittest.TestCase):
    def test_android_identity_survives_host_staging(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            tree = base / "tree"
            app = tree / "data/user_de/0/example"
            app.mkdir(parents=True)
            (app / "preferences").write_bytes(b"persisted application state")
            (app / "alias").symlink_to("preferences")
            (app / "old-socket").touch()

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
            identities = inode_metadata(source, "/data")
            self.assertNotIn("user_de/0/example/old-socket", identities)
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
            self.assertNotEqual(inode_metadata(destination, "/data"), identities)
            restore_inode_metadata(destination, "/data", inode_metadata(source, "/data"))
            self.assertEqual(inode_metadata(destination, "/data"), identities)
            content = subprocess.run(["debugfs", "-R", "cat /data/user_de/0/example/preferences",
                                      str(destination)], check=True, capture_output=True).stdout
            self.assertEqual(content, b"persisted application state")
            self.assertEqual((dumped / "data/user_de/0/example/alias").readlink(), Path("preferences"))
