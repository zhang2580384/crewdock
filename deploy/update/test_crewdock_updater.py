import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from crewdock_updater import (
    UpdateError,
    Updater,
    latest_release,
    parse_env_file,
    prune_release_backups,
    verify_release_manifests,
    version_tuple,
)


class UpdaterTests(unittest.TestCase):
    def test_release_tags_are_strict_and_ordered(self) -> None:
        self.assertEqual((1, 12, 3), version_tuple("v1.12.3"))
        self.assertEqual("v2.0.0", latest_release(["latest", "v1.9.9", "v2.0.0", "v2.0.0-rc.1"]))
        for invalid in ("1.2.3", "v01.2.3", "v1.2", "v1.2.3;touch /tmp/x"):
            with self.assertRaises(UpdateError):
                version_tuple(invalid)

    def test_env_parser_does_not_execute_values(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ".env"
            path.write_text("A=plain\nB='space value'\nC=$(touch /tmp/not-run)\n", encoding="utf-8")
            values = parse_env_file(path)
        self.assertEqual("plain", values["A"])
        self.assertEqual("space value", values["B"])
        self.assertEqual("$(touch /tmp/not-run)", values["C"])

    def test_release_requires_all_product_manifests_to_match(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "backend").mkdir()
            (root / "frontend").mkdir()
            (root / "backend" / "Cargo.toml").write_text(
                '[package]\nname = "crewdock-api"\nversion = "0.5.0"\n', encoding="utf-8"
            )
            (root / "frontend" / "package.json").write_text(
                json.dumps({"version": "0.5.0"}), encoding="utf-8"
            )
            (root / "frontend" / "package-lock.json").write_text(
                json.dumps({"version": "0.5.0", "packages": {"": {"version": "0.5.0"}}}),
                encoding="utf-8",
            )
            verify_release_manifests(root, "v0.5.0")
            with self.assertRaises(UpdateError):
                verify_release_manifests(root, "v0.6.0")

    def test_backup_retention_keeps_only_latest_controlled_release_backup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            backups = Path(directory) / "backups"
            keep = backups / "pre-v0.7.0-20260725-083000"
            old_timestamp = backups / "pre-v0.6.0-20260725-070000"
            old_legacy = backups / "pre-v0.1.1-7a8d366"
            unrelated = backups / "20260718-company-multidepartment"
            invalid = backups / "pre-v0.7.0-manual"
            for path in (keep, old_timestamp, old_legacy, unrelated, invalid):
                path.mkdir(parents=True, exist_ok=True)

            removed = prune_release_backups(backups, keep)

            self.assertEqual(
                ["pre-v0.1.1-7a8d366", "pre-v0.6.0-20260725-070000"],
                removed,
            )
            self.assertTrue(keep.is_dir())
            self.assertTrue(unrelated.is_dir())
            self.assertTrue(invalid.is_dir())

    def test_backup_retention_rejects_keep_directory_outside_backup_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            backups = root / "backups"
            keep = root / "pre-v0.7.0-20260725-083000"
            backups.mkdir()
            keep.mkdir()
            with self.assertRaises(UpdateError):
                prune_release_backups(backups, keep)

    def test_commands_use_docker_config_inside_update_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            updater = Updater(root)
            updater.ensure_layout()
            with patch("crewdock_updater.subprocess.run") as run:
                run.return_value.returncode = 0
                run.return_value.stdout = ""
                updater.command(["docker", "version"], capture=True)

            self.assertEqual(
                str(root / "update" / "docker-config"),
                run.call_args.kwargs["env"]["DOCKER_CONFIG"],
            )
            self.assertTrue((root / "update" / "docker-config").is_dir())


if __name__ == "__main__":
    unittest.main()
