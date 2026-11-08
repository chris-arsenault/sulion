"""Destructive maintenance must respect run age, activity, and source ownership."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "maintenance", Path(__file__).parents[1] / "storage-maintenance.py"
)
maintenance = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(maintenance)


class ArtifactCleanupTests(unittest.TestCase):
    def setUp(self):
        logging_config = patch.object(maintenance.logging, "basicConfig")
        logging_config.start()
        self.addCleanup(logging_config.stop)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.root = self.repo / ".artifacts"
        self.root.mkdir()
        self.cutoff = time.time() - 21 * maintenance.DAY
        self.old = self.cutoff - maintenance.DAY

    def file(self, path, text="payload"):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def age(self, path):
        for base, dirs, files in os.walk(path):
            for name in dirs + files:
                os.utime(Path(base) / name, (self.old, self.old), follow_symlinks=False)
        os.utime(path, (self.old, self.old), follow_symlinks=False)

    def test_new_descendant_preserves_entire_run_and_preview_never_deletes(self):
        run = self.root / "old-run"
        payload = self.file(run / "nested/checkpoint.bin")
        self.age(run)
        size = maintenance.expire(run, set(), [], self.cutoff, False)
        self.assertGreater(size, 0)
        self.assertTrue(payload.exists())
        os.utime(payload, None)
        self.assertEqual(maintenance.expire(run, set(), [], self.cutoff, True), 0)
        self.assertTrue(payload.exists())

    def test_expired_payload_removed_but_authored_and_registry_records_survive(self):
        run = self.root / "run"
        payload = self.file(run / "checkpoint.bin")
        tracked = self.file(run / "authored.json")
        keep = [
            tracked,
            self.file(run / "FINDINGS.md"),
            self.file(run / "probe.py"),
            self.file(run / "manifest.json"),
            self.file(run / "ledger.db"),
        ]
        self.age(run)
        self.assertGreater(maintenance.expire(run, {tracked}, [], self.cutoff, True), 0)
        self.assertFalse(payload.exists())
        self.assertTrue(all(p.exists() for p in keep))

    def test_live_cwd_or_open_file_preserves_old_run(self):
        run = self.root / "run"
        payload = self.file(run / "checkpoint.bin")
        self.age(run)
        for active in ([run], [payload]):
            self.assertEqual(
                maintenance.expire(run, set(), active, self.cutoff, True), 0
            )
        self.assertTrue(payload.exists())

    def test_process_scan_observes_live_run_cwd_and_open_checkpoint(self):
        run = self.root / "run"
        payload = self.file(run / "checkpoint.bin")
        process = subprocess.Popen(
            [
                "python3",
                "-c",
                "import time; f=open('checkpoint.bin'); print('ready',flush=True); time.sleep(30)",
            ],
            cwd=run,
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            self.assertEqual(process.stdout.readline().strip(), "ready")
            paths = json.loads(
                maintenance.command(["python3", "-c", maintenance.PROC_PATHS])
            )
            self.assertIn(str(run), paths)
            self.assertIn(str(payload), paths)
        finally:
            process.terminate()
            process.wait()
            process.stdout.close()

    def test_activity_inspection_failure_preserves_artifacts_and_reports_failure(self):
        payload = self.file(self.root / "run/checkpoint.bin")
        with patch(
            "sys.argv", ["storage-maintenance", "--home", str(self.repo), "--apply"]
        ):
            with patch.object(
                maintenance, "active_paths", side_effect=PermissionError("proc")
            ):
                with patch.object(maintenance, "clean_artifacts") as clean:
                    with patch.object(maintenance, "cache_cleanup", return_value=True):
                        with self.assertLogs(maintenance.LOG, level="ERROR"):
                            self.assertEqual(maintenance.main(), 1)
        clean.assert_not_called()
        self.assertTrue(payload.exists())

    def test_symlinks_never_followed_in_runs_or_roots(self):
        outside = self.file(self.repo / "saved-world.bin")
        run = self.root / "run"
        self.file(run / "trace.json")
        (run / "world").symlink_to(outside)
        self.age(run)
        maintenance.expire(run, set(), [], self.cutoff, True)
        self.assertTrue(outside.exists())
        self.assertTrue((run / "world").is_symlink())
        alias = self.repo / "alias"
        alias.symlink_to(self.root, target_is_directory=True)
        self.assertEqual(
            maintenance.clean_root(alias, self.repo, set(), [], self.cutoff, True), 0
        )

    def registry_fixture(self, status="completed"):
        root = self.root / "monte-carlo"
        run = root / "run"
        self.file(run / "telemetry.duckdb")
        self.file(run / "manifest.json", json.dumps({"status": status}))
        self.file(root / "registry.duckdb")
        self.file(self.root / "bin/MonteCarlo/Release/net8.0/MonteCarlo.dll")
        self.age(run)
        return root, run

    def test_missing_registry_preserves_monte_carlo_payload(self):
        payload = self.file(self.root / "monte-carlo/run/telemetry.duckdb")
        self.file(self.root / "bin/MonteCarlo/Release/net8.0/MonteCarlo.dll")
        self.age(self.root / "monte-carlo")
        with self.assertRaisesRegex(RuntimeError, "registry missing"):
            maintenance.clean_root(self.root, self.repo, set(), [], self.cutoff, True)
        self.assertTrue(payload.exists())

    def test_unrelated_monte_carlo_folder_uses_ordinary_retention(self):
        payload = self.file(self.root / "monte-carlo/run/sample.json")
        self.age(self.root / "monte-carlo")
        self.assertGreater(
            maintenance.clean_root(self.root, self.repo, set(), [], self.cutoff, True),
            0,
        )
        self.assertFalse(payload.exists())

    def test_registry_cli_marks_deleted_before_payload_removal_and_keeps_manifest(self):
        root, run = self.registry_fixture()
        destination = root / ".deleted/run-timestamp"
        calls = []

        def cli(args, **_kwargs):
            calls.append(args[2])
            if args[2] == "list":
                return f"run\tcompleted\t2026-01-01\t{run}\n"
            if args[2] == "delete-run":
                self.assertTrue((run / "telemetry.duckdb").exists())
                destination.parent.mkdir()
                shutil.move(run, destination)
                return ""
            return json.dumps({"artifactPath": str(destination)})

        with patch.object(maintenance, "command", side_effect=cli):
            reclaimed = maintenance.monte_carlo(
                root, self.repo, set(), [], self.cutoff, True
            )
        self.assertEqual(calls, ["list", "delete-run", "show"])
        self.assertGreater(reclaimed, 0)
        self.assertFalse((destination / "telemetry.duckdb").exists())
        self.assertTrue((destination / "manifest.json").exists())
        self.assertTrue((root / "registry.duckdb").exists())

    def test_registry_failure_unfinished_and_unknown_runs_are_preserved(self):
        root, run = self.registry_fixture()
        for listing in ("", f"run\trunning\t2026-01-01\t{run}\n"):
            with patch.object(maintenance, "command", return_value=listing):
                self.assertEqual(
                    maintenance.monte_carlo(
                        root, self.repo, set(), [], self.cutoff, True
                    ),
                    0,
                )
            self.assertTrue((run / "telemetry.duckdb").exists())
        with patch.object(
            maintenance,
            "command",
            side_effect=subprocess.CalledProcessError(1, "dotnet"),
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                maintenance.monte_carlo(root, self.repo, set(), [], self.cutoff, True)
        self.assertTrue((run / "telemetry.duckdb").exists())

    def test_legacy_registry_path_still_cleans_current_payload_without_editing_manifest(
        self,
    ):
        root, run = self.registry_fixture()
        original_manifest = (run / "manifest.json").read_bytes()
        destination = root / ".deleted/run-timestamp"

        def cli(args, **_kwargs):
            if args[2] == "list":
                return "run\tcompleted\t2026-01-01\t/home/dev/old/run\n"
            if args[2] == "delete-run":
                self.file(destination / "manifest.json")
                return ""
            return json.dumps({"artifactPath": str(destination)})

        with patch.object(maintenance, "command", side_effect=cli):
            maintenance.monte_carlo(root, self.repo, set(), [], self.cutoff, True)
        self.assertFalse((run / "telemetry.duckdb").exists())
        self.assertEqual((run / "manifest.json").read_bytes(), original_manifest)

    def test_existing_deleted_payload_expires_without_removing_tombstone(self):
        root, run = self.registry_fixture()
        deleted = root / ".deleted/previous-run"
        payload = self.file(deleted / "telemetry.duckdb")
        manifest = self.file(deleted / "manifest.json")
        self.age(deleted)
        with patch.object(maintenance, "command", return_value=""):
            maintenance.monte_carlo(root, self.repo, set(), [], self.cutoff, True)
        self.assertFalse(payload.exists())
        self.assertTrue(manifest.exists())
        self.assertTrue((run / "telemetry.duckdb").exists())

    def test_cache_failure_does_not_skip_other_commands_or_mark_weekly_complete(self):
        state = self.repo / ".cache/sulion"
        state.mkdir(parents=True)
        with patch.object(
            maintenance,
            "command",
            side_effect=[subprocess.CalledProcessError(1, "docker"), "", "", ""],
        ) as command:
            self.assertFalse(maintenance.cache_cleanup(self.repo, True))
        self.assertEqual(command.call_count, 4)
        self.assertFalse((state / "storage-maintenance-weekly").exists())


if __name__ == "__main__":
    unittest.main()
