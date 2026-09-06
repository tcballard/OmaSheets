"""Package installs must not delegate lifecycle operations to the custom setup."""
import contextlib
import io
import unittest
from unittest.mock import patch

from omasheets.cli import main


class PackageInstallTests(unittest.TestCase):
    @patch("omasheets.package_install.is_package_managed", return_value=True)
    @patch("subprocess.run")
    def test_update_explains_package_channels_without_running_installer(self, run, managed):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(main(["update"]), 0)
        self.assertIn("pacman -U", output.getvalue())
        run.assert_not_called()

    @patch("omasheets.package_install.is_package_managed", return_value=True)
    @patch("omasheets.installation.uninstall")
    def test_uninstall_does_not_remove_user_state_for_system_package(self, uninstall, managed):
        with self.assertRaisesRegex(SystemExit, "pacman -Rns"):
            main(["uninstall"])
        uninstall.assert_not_called()

    @patch("omasheets.package_install.is_package_managed", return_value=False)
    @patch("omasheets.installation.uninstall")
    def test_migration_requires_installed_package(self, uninstall, managed):
        with self.assertRaisesRegex(SystemExit, "Install the Arch package first"):
            main(["migrate-user-install"])
        uninstall.assert_not_called()

    @patch("omasheets.package_install.is_package_managed", return_value=True)
    @patch("omasheets.native_grid._service_socket_ready", return_value=True)
    @patch.dict("os.environ", {"XDG_RUNTIME_DIR": "/tmp/test-runtime"})
    @patch("omasheets.installation.uninstall")
    def test_migration_refuses_live_service(self, uninstall, ready, managed):
        with self.assertRaisesRegex(SystemExit, "Close OmaSheets"):
            main(["migrate-user-install"])
        uninstall.assert_not_called()
