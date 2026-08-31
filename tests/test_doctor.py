import unittest
from unittest.mock import Mock, patch

from omasheets.doctor import diagnose


class DoctorTests(unittest.TestCase):
    @patch("omasheets.doctor.IntegrationPaths.discover")
    @patch("omasheets.doctor.subprocess.run")
    @patch("omasheets.doctor.shutil.which")
    @patch("omasheets.doctor.Path.is_file")
    def test_required_runtime_controls_readiness(self, is_file, which, run, discover):
        is_file.side_effect = lambda: False
        which.side_effect = lambda name: f"/bin/{name}" if name != "soffice" else None
        run.return_value = Mock(returncode=0)
        discover.return_value = Mock(desktop=Mock(is_file=lambda: False), journal=Mock(is_file=lambda: False))
        result = diagnose()
        self.assertFalse(result["ready"])
        self.assertEqual([check["name"] for check in result["checks"][:4]], [
            "bwrap", "soffice", "python", "python-uno"
        ])


if __name__ == "__main__":
    unittest.main()
