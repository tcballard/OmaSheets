import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from omasheets.installation import InstallPaths, PLUGIN_ENTRY, install, uninstall
from omasheets.integration import DESKTOP_ID, IntegrationPaths


ROOT = Path(__file__).resolve().parents[1]


class InstallationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.paths = InstallPaths(
            app=root / "data/omasheets/app",
            build=root / "cache/omasheets/native-build",
            launcher=root / "home/.local/bin/omasheets",
            codex_plugin=root / "home/.codex/plugins/omasheets",
            codex_marketplace=root / "home/.agents/plugins/marketplace.json",
            journal=root / "state/omasheets/installation.json",
            integration=IntegrationPaths(
                root / "data/applications" / DESKTOP_ID,
                root / "config/mimeapps.list",
                root / "state/omasheets/desktop-integration.json",
            ),
        )
        self.stage = None

    def tearDown(self):
        self.temporary.cleanup()

    def fake_cmake(self, argv, **kwargs):
        if argv[1] == "-S":
            option = next(value for value in argv if value.startswith("-DCMAKE_INSTALL_PREFIX="))
            self.stage = Path(option.split("=", 1)[1])
        elif argv[1] == "--install":
            binary = self.stage / "bin"
            binary.mkdir(parents=True)
            for name in ("omasheets-window", "omasheets-lok-render"):
                path = binary / name
                path.write_bytes((name + "\n").encode())
                path.chmod(0o755)
        return subprocess.CompletedProcess(argv, 0)

    def test_install_and_uninstall_cover_all_owned_surfaces(self):
        result = install(ROOT, self.paths, check_dependencies=False, runner=self.fake_cmake)
        self.assertTrue(result["changed"])
        self.assertTrue(self.paths.launcher.is_file())
        self.assertTrue((self.paths.app / "bin/omasheets-window").is_file())
        mcp = json.loads((self.paths.codex_plugin / ".mcp.json").read_text())
        self.assertEqual(mcp["mcpServers"]["omasheets"]["command"], str(self.paths.launcher))
        marketplace = json.loads(self.paths.codex_marketplace.read_text())
        self.assertIn(PLUGIN_ENTRY, marketplace["plugins"])
        self.assertIn(str(self.paths.launcher), self.paths.integration.desktop.read_text())
        self.assertFalse(install(ROOT, self.paths, check_dependencies=False, runner=self.fake_cmake)["changed"])

        marketplace["plugins"].append({"name": "keep-me"})
        self.paths.codex_marketplace.write_text(json.dumps(marketplace))
        removed = uninstall(self.paths)
        self.assertEqual(removed["conflicts"], [])
        self.assertEqual(json.loads(self.paths.codex_marketplace.read_text())["plugins"], [{"name": "keep-me"}])
        self.assertFalse(self.paths.launcher.exists())
        self.assertFalse(self.paths.codex_plugin.exists())
        self.assertFalse(self.paths.app.exists())

    def test_uninstall_preserves_a_modified_owned_file(self):
        install(ROOT, self.paths, check_dependencies=False, runner=self.fake_cmake)
        self.paths.launcher.write_text("user changed this")
        result = uninstall(self.paths)
        self.assertEqual(len(result["conflicts"]), 1)
        self.assertTrue(self.paths.launcher.exists())
        self.assertTrue(self.paths.journal.exists())


if __name__ == "__main__":
    unittest.main()
