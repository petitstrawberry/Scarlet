#!/usr/bin/env python3
"""Host-only bootstrap tests: no network, SDK, Java, or JDK compilation needed.

Run: python3 -B -m unittest discover -s tools/openjdk/tests -p test_build.py -v
"""

import importlib.util
import hashlib
import io
import json
import os
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
from unittest import mock


spec = importlib.util.spec_from_file_location("openjdk_build", Path(__file__).resolve().parents[1] / "build.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cache = self.root / "cache"
        self.cache.mkdir()
        self.sources = self.root / "sources"
        self.sources.mkdir()
        self.env = os.environ.copy()
        quiet = mock.patch.object(build, "log")
        quiet.start()
        self.addCleanup(quiet.stop)

    def archive(self, entries=None):
        path = self.root / "fixture.tar.gz"
        with tarfile.open(path, "w:gz") as tar:
            for name, kind, content in entries or [("upstream/hello", "file", b"original\n")]:
                member = tarfile.TarInfo(name)
                member.mode = 0o755
                if kind == "file":
                    member.size = len(content)
                    tar.addfile(member, io.BytesIO(content))
                else:
                    member.type = {"symlink": tarfile.SYMTYPE, "hardlink": tarfile.LNKTYPE,
                                   "fifo": tarfile.FIFOTYPE, "directory": tarfile.DIRTYPE}[kind]
                    member.linkname = content
                    tar.addfile(member)
        source = {"url": "https://example.invalid/pinned.tar.gz", "sha256": build.sha256(path),
                  "directory": "upstream", "version": "fixture"}
        return path, source

    def prepare(self, archive, source, patch=None, overlay=None):
        patches = {"0001-test.patch": patch} if patch is not None else None
        return build.prepare_source("jdk", source, archive, self.sources, self.env, patches, overlay)

    def patch(self, new=b"patched"):
        return b"--- a/hello\n+++ b/hello\n@@ -1 +1 @@\n-original\n+" + new + b"\n"

    def test_download_hash_mismatch_never_publishes(self):
        archive, source = self.archive()
        with mock.patch.object(build.urllib.request, "urlopen", return_value=io.BytesIO(b"corrupt")):
            with self.assertRaisesRegex(build.BuildError, "SHA-256 mismatch"):
                build.download(source, self.cache)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_corrupt_cached_download_is_preserved_and_not_used(self):
        archive, source = self.archive()
        cached = self.cache / (source["sha256"] + ".tar.gz")
        cached.write_bytes(b"user data")
        with mock.patch.object(build.urllib.request, "urlopen") as fetch:
            with self.assertRaisesRegex(build.BuildError, "SHA-256 mismatch"):
                build.download(source, self.cache)
            fetch.assert_not_called()
        self.assertEqual(cached.read_bytes(), b"user data")

    def test_verified_download_is_reused_without_network(self):
        archive, source = self.archive()
        with mock.patch.object(build.urllib.request, "urlopen", return_value=io.BytesIO(archive.read_bytes())) as fetch:
            first = build.download(source, self.cache)
            second = build.download(source, self.cache)
            self.assertEqual(first, second)
            self.assertEqual(first.read_bytes(), archive.read_bytes())
            fetch.assert_called_once()

    def test_unsafe_tar_members_rejected_before_any_extraction(self):
        bad_entries = [
            [("../escape", "file", b"bad")],
            [("upstream/../../escape", "file", b"bad")],
            [("/absolute", "file", b"bad")],
            [("other/file", "file", b"bad")],
            [("upstream/link", "symlink", "../../escape")],
            [("upstream/link", "hardlink", "upstream/hello")],
            [("upstream/fifo", "fifo", "")],
            [("upstream/hello", "file", b"duplicate")],
            [("upstream/hello/child", "file", b"bad")],
            [("upstream", "file", b"bad")],
        ]
        for entries in bad_entries:
            with self.subTest(entries=entries):
                archive, source = self.archive([("upstream/hello", "file", b"good")] + entries)
                destination = self.root / "extracted"
                with self.assertRaises(build.BuildError):
                    build.extract_archive(archive, destination, source["directory"])
                self.assertFalse(destination.exists())
        self.assertFalse((self.root / "escape").exists())

    def test_extraction_never_reuses_an_existing_directory(self):
        archive, source = self.archive()
        destination = self.root / "user-tree"
        destination.mkdir()
        (destination / "hello").write_bytes(b"user work")
        with self.assertRaises(FileExistsError):
            build.extract_archive(archive, destination, source["directory"])
        self.assertEqual((destination / "hello").read_bytes(), b"user work")

    def test_patch_and_overlay_are_applied_once_and_reused(self):
        archive, source = self.archive()
        overlay = {"src/scarlet.hpp": (b"native port\n", False)}
        with mock.patch.object(build, "run", wraps=build.run) as run:
            first = self.prepare(archive, source, self.patch(), overlay)
            mtime = (first / "hello").stat().st_mtime_ns
            second = self.prepare(archive, source, self.patch(), overlay)
            run.assert_called_once()
        self.assertEqual(first, second)
        self.assertEqual((second / "hello").read_bytes(), b"patched\n")
        self.assertEqual((second / "hello").stat().st_mtime_ns, mtime)
        self.assertEqual((second / "src/scarlet.hpp").read_bytes(), b"native port\n")

    def test_changed_and_removed_inputs_get_new_trees_without_reset(self):
        archive, source = self.archive()
        first = self.prepare(archive, source, self.patch(), {"old.hpp": (b"old", False)})
        second = self.prepare(archive, source, self.patch(), {"new.hpp": (b"new", False)})
        third = self.prepare(archive, source, self.patch(b"revised"))
        self.assertEqual(len({first, second, third}), 3)
        self.assertEqual((first / "old.hpp").read_bytes(), b"old")
        self.assertFalse((second / "old.hpp").exists())
        self.assertEqual((second / "new.hpp").read_bytes(), b"new")
        self.assertEqual((first / "hello").read_bytes(), b"patched\n")
        self.assertEqual((third / "hello").read_bytes(), b"revised\n")

    def test_local_source_edits_are_reported_and_preserved(self):
        archive, source = self.archive()
        tree = self.prepare(archive, source)
        (tree / "hello").write_bytes(b"user change")
        with self.assertRaisesRegex(build.BuildError, "sources were modified"):
            self.prepare(archive, source)
        self.assertEqual((tree / "hello").read_bytes(), b"user change")

    def test_failed_patch_does_not_publish_or_leave_partial_source(self):
        archive, source = self.archive()
        bad_patch = self.patch().replace(b"-original", b"-not-an-anchor")
        with self.assertRaisesRegex(build.BuildError, "Command failed"):
            self.prepare(archive, source, bad_patch)
        self.assertTrue(all(p.name.endswith("-patch.log") for p in self.sources.iterdir()))
        self.assertEqual((self.prepare(archive, source) / "hello").read_bytes(), b"original\n")

    def test_patch_series_is_sorted_snapshotted_and_applied_completely(self):
        archive, source = self.archive()
        patches_dir = self.root / "patches"
        patches_dir.mkdir()
        first = self.patch()
        second = self.patch(b"platform").replace(b"-original", b"-patched").rstrip(b"\n")
        # Creation order differs from application order. Non-patches are ignored.
        (patches_dir / "0002-java-platform.patch").write_bytes(second)
        (patches_dir / "0001-scarlet.patch").write_bytes(first)
        (patches_dir / "README").write_text("not a patch")
        patches = build.snapshot_patches(patches_dir)
        self.assertEqual(list(patches), ["0001-scarlet.patch", "0002-java-platform.patch"])
        (patches_dir / "0002-java-platform.patch").write_text("changed by another task")
        with mock.patch.object(build, "run", wraps=build.run) as run:
            tree = build.prepare_source("jdk", source, archive, self.sources, self.env, patches)
            reused = build.prepare_source("jdk", source, archive, self.sources, self.env, patches)
            run.assert_called_once()
        self.assertEqual(tree, reused)
        self.assertEqual((tree / "hello").read_bytes(), b"platform\n")
        receipt = json.loads((tree.parent / "source.json").read_text())
        self.assertEqual(receipt["inputs"]["patches"], {
            "0001-scarlet.patch": hashlib.sha256(first).hexdigest(),
            "0002-java-platform.patch": hashlib.sha256(second).hexdigest(),
        })
        self.assertEqual(receipt["inputs"]["patch_sha256"], build.sha256(tree.parent / "scarlet.patch"))

    def test_patch_series_add_edit_rename_and_remove_change_source_identity(self):
        archive, source = self.archive()
        patches_dir = self.root / "patches"
        patches_dir.mkdir()
        (patches_dir / "0001-scarlet.patch").write_bytes(self.patch())

        def prepare_series():
            return build.prepare_source("jdk", source, archive, self.sources, self.env,
                                        build.snapshot_patches(patches_dir))

        first = prepare_series()
        second_patch = patches_dir / "0002-java-platform.patch"
        second_patch.write_bytes(self.patch(b"platform").replace(b"-original", b"-patched"))
        added = prepare_series()
        second_patch.write_bytes(self.patch(b"updated platform").replace(b"-original", b"-patched"))
        edited = prepare_series()
        renamed_patch = patches_dir / "0003-java-platform.patch"
        second_patch.rename(renamed_patch)
        renamed = prepare_series()
        self.assertEqual(len({first, added, edited, renamed}), 4)
        renamed_patch.unlink()
        self.assertEqual(prepare_series(), first)
        for tree, expected in ((first, b"patched\n"), (added, b"platform\n"),
                               (edited, b"updated platform\n"), (renamed, b"updated platform\n")):
            self.assertEqual((tree / "hello").read_bytes(), expected)

    def test_failure_in_later_patch_does_not_publish_partially_patched_tree(self):
        archive, source = self.archive()
        patches = {"0001-scarlet.patch": self.patch(), "0002-broken.patch": self.patch(b"wrong base")}
        with self.assertRaisesRegex(build.BuildError, "Command failed"):
            build.prepare_source("jdk", source, archive, self.sources, self.env, patches)
        self.assertTrue(all(p.name.endswith("-patch.log") for p in self.sources.iterdir()))

    def test_patch_snapshot_rejects_missing_empty_or_symlink_inputs(self):
        patches_dir = self.root / "patches"
        with self.assertRaises(build.BuildError):
            build.snapshot_patches(patches_dir)
        patches_dir.mkdir()
        with self.assertRaisesRegex(build.BuildError, "No patches"):
            build.snapshot_patches(patches_dir)
        path = patches_dir / "0001.patch"
        path.write_bytes(b"")
        with self.assertRaisesRegex(build.BuildError, "Empty patch"):
            build.snapshot_patches(patches_dir)
        path.unlink()
        other = self.root / "other.patch"
        other.write_bytes(self.patch())
        path.symlink_to(other)
        with self.assertRaisesRegex(build.BuildError, "regular file"):
            build.snapshot_patches(patches_dir)

    def test_output_ownership_and_lock(self):
        user = self.root / "user"
        user.mkdir()
        (user / "notes").write_text("keep")
        with self.assertRaisesRegex(build.BuildError, "not owned"):
            with build.output_lock(user):
                self.fail("must not acquire arbitrary output")
        owned = self.root / "owned"
        with build.output_lock(owned):
            with self.assertRaisesRegex(build.BuildError, "Another driver"):
                with build.output_lock(owned):
                    self.fail("must not acquire twice")
        with build.output_lock(owned):
            pass
        link = self.root / "link"
        link.symlink_to(user, target_is_directory=True)
        with self.assertRaisesRegex(build.BuildError, "symlink"):
            with build.output_lock(link):
                self.fail("must not follow output symlinks")
        self.assertEqual((user / "notes").read_text(), "keep")

    def test_compiler_drivers_are_distinct_regular_executable_copies(self):
        data = (build.HERE / "cc.py").read_bytes()
        cc, cxx = build.compiler_drivers(self.root, data)
        for path in (cc, cxx):
            self.assertFalse(path.is_symlink())
            self.assertTrue(os.access(path, os.X_OK))
            self.assertEqual(path.read_bytes(), data)
        self.assertEqual((cc.name, cxx.name), ("clang", "clang++"))
        self.assertNotEqual(cc.stat().st_ino, cxx.stat().st_ino)
        build.compiler_drivers(self.root, data)
        cxx.unlink()
        cxx.symlink_to(cc)
        with self.assertRaises(build.BuildError):
            build.compiler_drivers(self.root, data)
        self.assertEqual(cc.read_bytes(), data)

    def test_hotspot_configure_contract(self):
        env = {"SCARLET_NATIVE_" + name: "/sdk/" + name.lower() for name in ("AR", "NM", "RANLIB")}
        boot = self.root / "Java 21"
        with mock.patch.object(build, "run") as run:
            build.configure_hotspot(self.root / "jdk", self.root / "build", boot,
                                    self.root / "ffi", self.root / "clang", self.root / "clang++", env, self.root / "log")
        command = [str(arg) for arg in run.call_args.args[0]]
        required = [f"--with-boot-jdk={boot}", f"--with-build-jdk={boot}",
                    "--with-freetype=bundled", f"--with-libffi={self.root / 'ffi'}",
                    "--openjdk-target=aarch64-unknown-scarlet", "--with-jvm-variants=zero",
                    "--enable-static-build", "--enable-headless-only",
                    "--with-jvm-features=serialgc,-epsilongc,-g1gc,-parallelgc,-shenandoahgc,-jvmti,-jfr,-management,-services,-vm-structs,-cds"]
        for arg in required:
            self.assertIn(arg, command)
        self.assertFalse(any("/nix/store" in arg for arg in command))

    def test_c1_configure_uses_aarch64_compiler_instead_of_zero(self):
        env = {"SCARLET_NATIVE_" + name: "/sdk/" + name.lower() for name in ("AR", "NM", "RANLIB")}
        with mock.patch.object(build, "run") as run:
            build.configure_hotspot(self.root / "jdk", self.root / "build", self.root / "jdk21",
                                    self.root / "ffi", self.root / "clang", self.root / "clang++",
                                    env, self.root / "log", "c1")
        command = [str(arg) for arg in run.call_args.args[0]]
        self.assertIn("--with-jvm-variants=server", command)
        self.assertIn("--with-jvm-features=compiler1,-compiler2," + build.JVM_FEATURES, command)
        self.assertNotIn("--with-jvm-variants=zero", command)

    def test_jdk25_uses_upstream_static_library_make_target(self):
        env = {"SCARLET_NATIVE_" + name: "/sdk/" + name.lower() for name in ("AR", "NM", "RANLIB")}
        with mock.patch.object(build, "run") as run:
            build.configure_hotspot(self.root / "jdk", self.root / "build", self.root / "jdk25",
                                    self.root / "ffi", self.root / "clang", self.root / "clang++",
                                    env, self.root / "log", "c1", 25)
        command = [str(arg) for arg in run.call_args.args[0]]
        self.assertNotIn("--enable-static-build", command)
        self.assertIn("--with-jvm-variants=server", command)

    def test_image_version_comes_from_upstream_release_numbers(self):
        config = self.root / "make/conf/version-numbers.conf"
        config.parent.mkdir(parents=True)
        names = ("FEATURE", "INTERIM", "UPDATE", "PATCH", "EXTRA1", "EXTRA2", "EXTRA3")
        for parts, expected in (((21, 0, 13, 0, 0, 0, 0), (21, "21.0.13")),
                                ((25, 0, 5, 0, 2, 0, 0), (25, "25.0.5.0.2"))):
            config.write_text("".join(f"DEFAULT_VERSION_{key}={value}\n"
                                      for key, value in zip(names, parts)))
            self.assertEqual(build.source_version(self.root), expected)
        config.write_text("DEFAULT_VERSION_FEATURE=25\n")
        with self.assertRaises(build.BuildError):
            build.source_version(self.root)

    @staticmethod
    def elf(osabi=0x53, interpreter=False):
        data = bytearray(256)
        data[:8] = b"\x7fELF\x02\x01\x01" + bytes([osabi])
        struct.pack_into("<HHIQQQIHHHHHH", data, 16, 2, 183, 1, 0x400000 + 240,
                         64, 0, 0, 64, 56, 2 if interpreter else 1, 0, 0, 0)
        struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, 0, 0x400000, 0, 256, 256, 4096)
        if interpreter:
            data[200:211] = b"/ld-host\x00\x00\x00"
            struct.pack_into("<IIQQQQQQ", data, 120, 3, 4, 200, 0, 0, 11, 11, 1)
        return data

    def test_elf_audit_accepts_static_scarlet_and_rejects_host_or_dynamic(self):
        probe = self.root / "probe"
        logfile = self.root / "audit.json"
        probe.write_bytes(self.elf())
        report = build.audit_probe(probe, logfile, self.env)
        self.assertEqual(report["osabi"], 0x53)
        self.assertEqual(report["machine"], "aarch64")
        probe.write_bytes(self.elf(osabi=0))
        with self.assertRaises(build.BuildError):
            build.audit_probe(probe, logfile, self.env)
        probe.write_bytes(self.elf(interpreter=True))
        with self.assertRaisesRegex(build.BuildError, "static Scarlet"):
            build.audit_probe(probe, logfile, self.env)

    def support_fixture(self, osabi=0x53, fail_rust=False):
        """Exercise the CLI and real ELF auditor with fake compilation only."""
        here = self.root / "openjdk"
        here.mkdir()
        (here / "tests").mkdir()
        (here / "cc.py").write_bytes((build.HERE / "cc.py").read_bytes())
        for name in ("tests/runtime-support.cpp", "runtime-support.c", "runtime-bridge.rs"):
            (here / name).write_text(f"fixture: {name}\n")
        # Deliberately omit sources.json, patches, overlay and Java. This target
        # must work without accessing them or building any archive dependency.
        self.support_commands = []
        real_run = build.run

        def fake_compile(command, cwd, logfile, env):
            command = [str(arg) for arg in command]
            if str(build.ROOT / "tools/elf_audit.py") in command:
                return real_run(command, cwd, logfile, env)
            self.support_commands.append(command)
            if fail_rust and command[0] == "/sdk/rustc":
                raise build.BuildError("Rust compilation failed")
            output = Path(command[command.index("-o") + 1])
            output.write_bytes(self.elf(osabi) if output.name == "runtime-support-probe" else b"compiled fixture")

        env = dict(self.env, SCARLET_RUSTC="/sdk/rustc")
        for patch in (mock.patch.object(build, "HERE", here),
                      mock.patch.object(build, "native_environment", return_value=(env, {"fixture": True})),
                      mock.patch.object(build, "run", side_effect=fake_compile)):
            patch.start()
            self.addCleanup(patch.stop)
        for name in ("load_sources", "snapshot_patches", "download", "prepare_source", "build_libffi",
                     "validate_boot_jdk", "configure_hotspot"):
            patch = mock.patch.object(build, name, side_effect=AssertionError(f"Unexpected dependency: {name}"))
            patch.start()
            self.addCleanup(patch.stop)
        return here

    def run_support(self):
        output = self.root / "support-output"
        result = build.main(["--target", "runtime-support-probe", "--output", str(output)])
        return result, output

    def test_runtime_support_cli_uses_c_cxx_and_rust_without_libffi_or_jdk(self):
        here = self.support_fixture()
        result, output = self.run_support()
        self.assertEqual(result, 0)
        receipt = json.loads((output / "runtime-support-probe.json").read_text())
        self.assertNotIn("libffi", receipt)
        self.assertFalse((output / "downloads").exists())
        self.assertFalse((output / "sources").exists())
        self.assertEqual(receipt["inputs"]["sources"], {})
        self.assertEqual(receipt["inputs"]["patches"], {})
        self.assertEqual(receipt["inputs"]["local_sources"], {
            name: hashlib.sha256((here / name).read_bytes()).hexdigest()
            for name in ("tests/runtime-support.cpp", "runtime-support.c", "runtime-bridge.rs")})
        self.assertEqual(receipt["elf"]["sha256"], build.sha256(Path(receipt["artifact"])))
        self.assertIn("not executed", receipt["scope"])
        cc, rust, cxx = self.support_commands
        self.assertEqual(Path(cc[0]).name, "clang")
        self.assertIn("-c", cc)
        self.assertEqual(Path(cc[cc.index("-c") + 1]).name, "runtime-support.c")
        self.assertEqual(rust[:8], ["/sdk/rustc", "--crate-type", "staticlib", "--target",
                                   "aarch64-unknown-scarlet", "-C", "panic=abort", "-O"])
        self.assertEqual(Path(rust[8]).name, "runtime-bridge.rs")
        self.assertEqual(Path(cxx[0]).name, "clang++")
        for filename in ("runtime-support.cpp", "runtime-support.o", "libscarlet_jvm_bridge.a"):
            self.assertIn(filename, [Path(arg).name for arg in cxx])

    def test_all_runtime_support_source_hashes_invalidate_build_without_overwriting_snapshots(self):
        here = self.support_fixture()

        def compile_probe():
            result, output = self.run_support()
            self.assertEqual(result, 0)
            return json.loads((output / "runtime-support-probe.json").read_text())

        first = compile_probe()
        self.assertEqual(compile_probe()["work"], first["work"])
        prior = first
        for name in ("tests/runtime-support.cpp", "runtime-support.c", "runtime-bridge.rs"):
            old_data = (here / name).read_bytes()
            (here / name).write_bytes(old_data + b"modified\n")
            current = compile_probe()
            self.assertNotEqual(current["work"], prior["work"])
            self.assertNotEqual(current["inputs"]["local_sources"][name], prior["inputs"]["local_sources"][name])
            self.assertEqual((Path(prior["work"]) / Path(name).name).read_bytes(), old_data)
            self.assertEqual((Path(current["work"]) / Path(name).name).read_bytes(), old_data + b"modified\n")
            prior = current

    def test_runtime_support_compiler_failure_stops_without_success_receipt(self):
        self.support_fixture(fail_rust=True)
        with mock.patch.object(build.sys, "stderr", new=io.StringIO()):
            result, output = self.run_support()
        self.assertEqual(result, 1)
        self.assertEqual(len(self.support_commands), 2)
        self.assertFalse((output / "runtime-support-probe.json").exists())
        self.assertEqual(list((output / "builds").glob("*/result.json")), [])

    def test_runtime_support_failed_elf_audit_does_not_publish_success_receipt(self):
        self.support_fixture(osabi=0)
        with mock.patch.object(build.sys, "stderr", new=io.StringIO()):
            result, output = self.run_support()
        self.assertEqual(result, 1)
        self.assertFalse((output / "runtime-support-probe.json").exists())
        self.assertEqual(list((output / "builds").glob("*/result.json")), [])


if __name__ == "__main__":
    unittest.main()
