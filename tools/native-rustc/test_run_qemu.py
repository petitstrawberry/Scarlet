import importlib.util
import contextlib
import io
import sqlite3
from pathlib import Path
import sys
import tempfile
import unittest


DIRECTORY = Path(__file__).resolve().parent
sys.path.insert(0, str(DIRECTORY))
SPEC = importlib.util.spec_from_file_location("native_rustc_run_qemu", DIRECTORY / "run-qemu.py")
assert SPEC and SPEC.loader
RUN_QEMU = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUN_QEMU)


class StagingLinkTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        (self.root / "bin").mkdir()
        (self.root / "lib").mkdir()
        (self.root / "lib/driver.so").write_text("driver")

    def tearDown(self):
        self.temporary.cleanup()

    def test_relative_file_link_within_staging_is_allowed(self):
        (self.root / "bin/driver.so").symlink_to("../lib/driver.so")
        RUN_QEMU.validate_staging_links(self.root)

    def test_absolute_link_is_rejected(self):
        outside = self.root.parent / (self.root.name + "-outside")
        outside.write_text("outside")
        self.addCleanup(outside.unlink)
        (self.root / "bin/driver.so").symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "must be relative"):
            RUN_QEMU.validate_staging_links(self.root)

    def test_relative_link_escaping_staging_is_rejected(self):
        outside = self.root.parent / (self.root.name + "-outside")
        outside.write_text("outside")
        self.addCleanup(outside.unlink)
        (self.root / "bin/driver.so").symlink_to(Path("../..") / outside.name)
        with self.assertRaisesRegex(ValueError, "escapes its root"):
            RUN_QEMU.validate_staging_links(self.root)

    def test_relative_link_to_staging_root_is_rejected(self):
        (self.root / "bin/root").symlink_to("../..")
        with self.assertRaisesRegex(ValueError, "escapes its root"):
            RUN_QEMU.validate_staging_links(self.root)

    def test_broken_link_is_rejected(self):
        (self.root / "bin/driver.so").symlink_to("../lib/missing.so")
        with self.assertRaisesRegex(ValueError, "invalid staged symlink"):
            RUN_QEMU.validate_staging_links(self.root)

    def test_old_runtime_copy_in_loader_search_path_is_rejected(self):
        name = "librustc_driver-fixture.so"
        (self.root / "bin" / name).write_bytes(b"old TLS layout")
        (self.root / "lib" / name).write_bytes(b"new TLS layout")
        with self.assertRaisesRegex(ValueError, "conflicting Rust runtime copies"):
            RUN_QEMU.validate_runtime_copies(self.root)
        (self.root / "bin" / name).write_bytes(b"new TLS layout")
        RUN_QEMU.validate_runtime_copies(self.root)


class CStartupEvidenceTests(unittest.TestCase):
    def test_zlib_requires_successful_status_and_exact_stdout_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with self.assertRaisesRegex(ValueError, "evidence is missing"):
                RUN_QEMU.validate_zlib_evidence(output)
            (output / "ZLIB_PASS").touch()
            (output / "zlib.status").write_text("exit=Some(1) elapsed_ms=5\n")
            (output / "zlib.stdout").write_bytes(RUN_QEMU.ZLIB_HELLO + b"\n")
            with self.assertRaisesRegex(ValueError, "required status 47"):
                RUN_QEMU.validate_zlib_evidence(output)
            (output / "zlib.status").write_text("exit=Some(47) elapsed_ms=5\n")
            for text in (b"", b"prefix" + RUN_QEMU.ZLIB_HELLO, RUN_QEMU.ZLIB_HELLO + b"suffix"):
                (output / "zlib.stdout").write_bytes(text)
                with self.assertRaisesRegex(ValueError, "success marker"):
                    RUN_QEMU.validate_zlib_evidence(output)
            (output / "zlib.stdout").write_bytes(RUN_QEMU.ZLIB_HELLO + b"\n")
            RUN_QEMU.validate_zlib_evidence(output)

    def test_marker_cannot_mask_missing_or_failed_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with self.assertRaisesRegex(ValueError, "evidence is missing"):
                RUN_QEMU.validate_c_startup_evidence(output)
            (output / "C_STARTUP_PASS").touch()
            with self.assertRaises(FileNotFoundError):
                RUN_QEMU.validate_c_startup_evidence(output)
            for status in ("exit=Some(0) ", "exit=Some(139) ", "exit=None "):
                (output / "c-startup.status").write_text(status)
                with self.assertRaisesRegex(ValueError, "required status 43"):
                    RUN_QEMU.validate_c_startup_evidence(output)
            (output / "c-startup.status").write_text("exit=Some(43) elapsed_ms=10 expected_exit=43\n")
            RUN_QEMU.validate_c_startup_evidence(output)


class SQLiteEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.output = Path(self.temporary.name)
        (self.output / "SQLITE_PASS").touch()
        for phase, (expected_exit, marker) in RUN_QEMU.SQLITE_PHASES.items():
            (self.output / f"{phase}.status").write_text(f"exit=Some({expected_exit}) elapsed_ms=5\n")
            (self.output / f"{phase}.stdout").write_bytes(marker + b"\n")
        for storage in ("ext2", "tmpfs"):
            with (self.output / f"sqlite-{storage}-create.stdout").open("ab") as stdout:
                stdout.write(RUN_QEMU.SQLITE_PTHREAD_HELLO + b"\n")
            (self.output / f"sqlite-{storage}-crash.journal").write_bytes(
                RUN_QEMU.SQLITE_JOURNAL_MAGIC + bytes(1024))
        self.database = self.output / "sqlite/sqlite-roundtrip.db"
        self.database.parent.mkdir()
        with contextlib.closing(sqlite3.connect(self.database)) as connection:
            connection.executescript("""
                CREATE TABLE groups(id INTEGER PRIMARY KEY, title TEXT NOT NULL);
                CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT NOT NULL UNIQUE,
                    score REAL NOT NULL, group_id INTEGER NOT NULL REFERENCES groups(id), payload BLOB);
                CREATE INDEX idx_items_group ON items(group_id);
            """)
            connection.executemany("INSERT INTO groups VALUES (?, ?)",
                                   [(index, f"group-{index}") for index in range(3)])
            connection.executemany("INSERT INTO items VALUES (?, ?, ?, ?, ?)",
                                   [(index, f"scarlet-{index:02d}-苺", index * 0.25, index % 3,
                                     RUN_QEMU.sqlite_expected_blob() if index == 7 else None)
                                    for index in range(1, 25)])
            connection.commit()

    def test_all_eight_processes_and_persisted_database_are_required(self):
        before = self.database.read_bytes()
        result = RUN_QEMU.validate_sqlite_evidence(self.output)
        self.assertEqual(result["integrity_check"], "ok")
        self.assertEqual(result["item_count"], 24)
        self.assertEqual(result["blob_bytes"], 131113)
        self.assertTrue(result["read_only"])
        self.assertTrue(result["process_exit_recovery_verified"])
        self.assertTrue(result["pthread_verified"])
        self.assertEqual(set(result["hot_journal_snapshots"]), {"ext2", "tmpfs"})
        self.assertEqual(self.database.read_bytes(), before)
        self.assertEqual(list(self.database.parent.iterdir()), [self.database])
        (self.output / "SQLITE_PASS").unlink()
        with self.assertRaisesRegex(ValueError, "evidence is missing"):
            RUN_QEMU.validate_sqlite_evidence(self.output)

    def test_marker_cannot_mask_any_failed_process(self):
        for phase, (expected_exit, _) in RUN_QEMU.SQLITE_PHASES.items():
            path = self.output / f"{phase}.status"
            for status in ("exit=Some(0) ", "exit=Some(139) ", "exit=None "):
                path.write_text(status)
                with self.assertRaisesRegex(ValueError, f"required status {expected_exit}"):
                    RUN_QEMU.validate_sqlite_evidence(self.output)
            path.write_text(f"exit=Some({expected_exit}) elapsed_ms=5\n")
            path.unlink()
            with self.assertRaises(FileNotFoundError):
                RUN_QEMU.validate_sqlite_evidence(self.output)
            path.write_text(f"exit=Some({expected_exit}) elapsed_ms=5\n")

    def test_both_filesystems_require_exact_pthread_acceptance(self):
        for storage in ("ext2", "tmpfs"):
            path = self.output / f"sqlite-{storage}-create.stdout"
            original = path.read_bytes()
            for marker in (b"", b"prefix" + RUN_QEMU.SQLITE_PTHREAD_HELLO,
                           RUN_QEMU.SQLITE_PTHREAD_HELLO + b"suffix"):
                path.write_bytes(RUN_QEMU.SQLITE_HELLO + b"\n" + marker + b"\n")
                with self.assertRaisesRegex(ValueError, "pthread success marker"):
                    RUN_QEMU.validate_sqlite_evidence(self.output)
            path.write_bytes(original)

    def test_every_process_requires_an_exact_stdout_marker(self):
        for phase, (_, marker) in RUN_QEMU.SQLITE_PHASES.items():
            path = self.output / f"{phase}.stdout"
            for text in (b"", b"prefix" + marker, marker + b"suffix"):
                path.write_bytes(text)
                with self.assertRaisesRegex(ValueError, "success marker"):
                    RUN_QEMU.validate_sqlite_evidence(self.output)
            path.write_bytes(marker + b"\n")

    def test_crash_and_recover_have_distinct_success_contracts(self):
        for storage in ("ext2", "tmpfs"):
            crash = self.output / f"sqlite-{storage}-crash"
            recover = self.output / f"sqlite-{storage}-recover"
            crash.with_suffix(".status").write_text("exit=Some(53) elapsed_ms=5\n")
            with self.assertRaisesRegex(ValueError, "required status 134"):
                RUN_QEMU.validate_sqlite_evidence(self.output)
            crash.with_suffix(".status").write_text("exit=Some(134) elapsed_ms=5\n")
            crash.with_suffix(".stdout").write_bytes(RUN_QEMU.SQLITE_HELLO + b"\n")
            with self.assertRaisesRegex(ValueError, "success marker"):
                RUN_QEMU.validate_sqlite_evidence(self.output)
            crash.with_suffix(".stdout").write_bytes(RUN_QEMU.SQLITE_CRASH_READY + b"\n")
            recover.with_suffix(".status").write_text("exit=Some(134) elapsed_ms=5\n")
            with self.assertRaisesRegex(ValueError, "required status 53"):
                RUN_QEMU.validate_sqlite_evidence(self.output)
            recover.with_suffix(".status").write_text("exit=Some(53) elapsed_ms=5\n")
            recover.with_suffix(".stdout").write_bytes(RUN_QEMU.SQLITE_CRASH_READY + b"\n")
            with self.assertRaisesRegex(ValueError, "success marker"):
                RUN_QEMU.validate_sqlite_evidence(self.output)
            recover.with_suffix(".stdout").write_bytes(RUN_QEMU.SQLITE_HELLO + b"\n")

    def test_crash_marker_cannot_replace_a_hot_journal_snapshot(self):
        for storage in ("ext2", "tmpfs"):
            journal = self.output / f"sqlite-{storage}-crash.journal"
            original = journal.read_bytes()
            journal.unlink()
            with self.assertRaises(FileNotFoundError):
                RUN_QEMU.validate_sqlite_evidence(self.output)
            for data in (b"", RUN_QEMU.SQLITE_JOURNAL_MAGIC + bytes(100), bytes(2048)):
                journal.write_bytes(data)
                with self.assertRaisesRegex(ValueError, "valid hot rollback journal"):
                    RUN_QEMU.validate_sqlite_evidence(self.output)
            journal.write_bytes(original)

    def test_markers_cannot_mask_a_missing_database(self):
        self.database.unlink()
        with self.assertRaisesRegex(ValueError, "database is missing"):
            RUN_QEMU.validate_sqlite_evidence(self.output)
        self.assertFalse(self.database.exists())

    def test_markers_cannot_mask_corrupted_database_bytes(self):
        self.database.write_bytes(b"not a SQLite database" + bytes(4096))
        with self.assertRaisesRegex(ValueError, "database validation failed"):
            RUN_QEMU.validate_sqlite_evidence(self.output)

    def test_integrity_ok_cannot_mask_incorrect_data(self):
        with contextlib.closing(sqlite3.connect(self.database)) as connection:
            connection.execute("UPDATE items SET label='wrong' WHERE id=24")
            connection.commit()
            self.assertEqual(connection.execute("PRAGMA integrity_check").fetchall(), [("ok",)])
        with self.assertRaisesRegex(ValueError, "rows or BLOB differ"):
            RUN_QEMU.validate_sqlite_evidence(self.output)

    def test_blob_bytes_are_checked_not_only_size(self):
        with contextlib.closing(sqlite3.connect(self.database)) as connection:
            connection.execute("UPDATE items SET payload=zeroblob(131113) WHERE id=7")
            connection.commit()
        with self.assertRaisesRegex(ValueError, "rows or BLOB differ"):
            RUN_QEMU.validate_sqlite_evidence(self.output)

    def test_unrecovered_journals_are_rejected_without_host_mutation(self):
        for suffix in ("-journal", "-wal", "-shm"):
            sidecar = self.database.with_name(self.database.name + suffix)
            sidecar.write_bytes(b"unrecovered journal")
            with self.assertRaisesRegex(ValueError, "unexpected nonempty"):
                RUN_QEMU.validate_sqlite_evidence(self.output)
            self.assertEqual(sidecar.read_bytes(), b"unrecovered journal")
            sidecar.unlink()


class SerialDiagnosticTests(unittest.TestCase):
    def test_panic_diagnostic_is_drained_and_late_success_cannot_override_failure(self):
        script = (
            "import sys,time; "
            "sys.stdout.write('panicked at'); sys.stdout.flush(); time.sleep(0.03); "
            "sys.stdout.write(' fixture.rs:42: assertion failed\\nSCARLET_LOADER_SMOKE_OK\\n'); "
            "sys.stdout.flush(); time.sleep(1)"
        )
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            captured = io.TextIOWrapper(io.BytesIO(), encoding="utf-8")
            with contextlib.redirect_stdout(captured):
                result = RUN_QEMU.smoke.run_guest([sys.executable, "-c", script], output, 3)
            self.assertEqual(result["result"], "guest panic")
            self.assertIn(b"fixture.rs:42: assertion failed", (output / "serial.log").read_bytes())


if __name__ == "__main__":
    unittest.main()
