import unittest
from unittest.mock import Mock, patch

from omasheets.doctor import diagnose


class DoctorTests(unittest.TestCase):
    def diagnose_owned_runtime(self, *, missing=None, package=False):
        runtime = {"checks": [{"name": "Qt Quick", "ok": True, "detail": "/usr/lib/libQt6Quick.so"}]}
        grid = {"ready": missing != "omasheets-grid", "detail": "installed grid"}
        integration = Mock(desktop=Mock(is_file=lambda: True), journal=Mock(is_file=lambda: True))
        with patch("omasheets.doctor.dependency_report", return_value=runtime) as dependencies, patch(
            "omasheets.doctor.grid_status", return_value=grid,
        ), patch("omasheets.doctor.is_package_managed", return_value=package), patch(
            "omasheets.doctor.IntegrationPaths.discover", return_value=integration,
        ), patch("omasheets.doctor.Path.is_file", return_value=package), patch(
            "omasheets.doctor.shutil.which", side_effect=lambda name: None if name == missing else f"/bin/{name}",
        ) as which:
            result = diagnose()
        dependencies.assert_called_once_with(include_setup=not package)
        self.assertEqual([call.args[0] for call in which.call_args_list], ["omasheets-kit", "omasheets-service"])
        return result

    def test_owned_kit_service_and_grid_are_ready_without_a_foreign_engine(self):
        result = self.diagnose_owned_runtime()
        self.assertTrue(result["ready"])
        names = [check["name"] for check in result["checks"]]
        self.assertIn("omasheets-kit", names)
        self.assertIn("omasheets-service", names)
        self.assertIn("omasheets-grid", names)
        for retired in ("soffice", "python-uno", "libreofficekit-engine", "omasheets-window"):
            self.assertNotIn(retired, names)

    def test_each_owned_executable_controls_readiness(self):
        for executable in ("omasheets-kit", "omasheets-service", "omasheets-grid"):
            with self.subTest(executable=executable):
                self.assertFalse(self.diagnose_owned_runtime(missing=executable)["ready"])

    def test_package_installation_omits_setup_runtime_check(self):
        self.assertTrue(self.diagnose_owned_runtime(package=True)["ready"])


if __name__ == "__main__":
    unittest.main()
