import subprocess
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from omasheets.errors import ConflictError
from omasheets.cli import build_parser
from omasheets.user_service import UserServicePaths, install, uninstall, unit_file


class UserServiceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        binary = root / 'app % "quoted"/omasheets-service'
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/bin/sh\nexit 0\n")
        binary.chmod(0o755)
        self.paths = UserServicePaths(
            binary=binary,
            unit=root / "config/systemd/user/omasheets-native.service",
            journal=root / "state/omasheets/user-service.json",
        )

    def tearDown(self):
        self.temporary.cleanup()

    @staticmethod
    def completed(*_arguments, **_keywords):
        return subprocess.CompletedProcess([], 0, "", "")

    @patch("omasheets.user_service._systemctl")
    def test_setup_is_opt_in_idempotent_and_reversible(self, systemctl):
        systemctl.side_effect = self.completed
        first = install(self.paths)
        self.assertTrue(first["changed"])
        self.assertFalse(first["enabled"])
        self.assertIn("ExecStart=\"", self.paths.unit.read_text())
        self.assertIn("%%", self.paths.unit.read_text())
        self.assertIn('\\"quoted\\"', self.paths.unit.read_text())
        systemctl.assert_called_once_with("daemon-reload")

        systemctl.reset_mock()
        second = install(self.paths, enable=True)
        self.assertFalse(second["changed"])
        self.assertTrue(second["enabled"])
        systemctl.assert_called_once_with("enable", "--now", "omasheets-native.service")

        systemctl.reset_mock()
        removed = uninstall(self.paths)
        self.assertTrue(removed["changed"])
        self.assertFalse(self.paths.unit.exists())
        self.assertFalse(self.paths.journal.exists())
        self.assertEqual(systemctl.call_args_list[0].args[:3], (
            "disable", "--now", "omasheets-native.service",
        ))
        self.assertEqual(systemctl.call_args_list[1].args, ("daemon-reload",))

    @patch("omasheets.user_service._systemctl")
    def test_modified_unit_is_stopped_but_preserved(self, systemctl):
        systemctl.side_effect = self.completed
        install(self.paths)
        self.paths.unit.write_text("user changed this")

        with self.assertRaisesRegex(ConflictError, "preserved"):
            uninstall(self.paths)

        self.assertTrue(self.paths.unit.exists())
        self.assertTrue(self.paths.journal.exists())

    @patch("omasheets.user_service._systemctl")
    def test_failed_enable_rolls_back_the_unit_and_journal(self, systemctl):
        systemctl.side_effect = [
            self.completed(),
            subprocess.CalledProcessError(1, ["systemctl"]),
            self.completed(),
            self.completed(),
        ]

        with self.assertRaises(subprocess.CalledProcessError):
            install(self.paths, enable=True)

        self.assertFalse(self.paths.unit.exists())
        self.assertFalse(self.paths.journal.exists())

    @patch("omasheets.user_service._systemctl")
    def test_stop_failure_preserves_everything(self, systemctl):
        systemctl.side_effect = self.completed
        install(self.paths)
        systemctl.side_effect = lambda *_arguments, **_keywords: subprocess.CompletedProcess(
            [], 2, "", "no user manager",
        )

        with self.assertRaisesRegex(ConflictError, "could not stop"):
            uninstall(self.paths)

        self.assertTrue(self.paths.unit.exists())
        self.assertTrue(self.paths.journal.exists())

    def test_unit_requires_an_installed_executable(self):
        missing = UserServicePaths(
            binary=self.paths.binary.with_name("missing"),
            unit=self.paths.unit,
            journal=self.paths.journal,
        )
        with self.assertRaisesRegex(RuntimeError, "missing or not executable"):
            install(missing)
        with self.assertRaises(FileNotFoundError):
            unit_file(missing.binary)

    def test_omarchy_setup_requires_explicit_service_opt_in(self):
        passive = build_parser().parse_args(["setup", "--omarchy"])
        self.assertFalse(passive.enable_service)
        enabled = build_parser().parse_args([
            "setup", "--omarchy", "--enable-service",
        ])
        self.assertTrue(enabled.enable_service)


if __name__ == "__main__":
    unittest.main()
