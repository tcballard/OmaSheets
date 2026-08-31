import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class PluginContractTests(unittest.TestCase):
    def test_manifest_points_to_the_review_widget(self):
        manifest = json.loads((ROOT / "manifest.json").read_text())
        self.assertEqual(manifest["schemaVersion"], 1)
        self.assertEqual(manifest["id"], "io.github.tcballard.omasheets")
        self.assertEqual(manifest["kinds"], ["bar-widget"])
        self.assertEqual(manifest["entryPoints"], {"barWidget": "Panel.qml"})

    def test_widget_never_builds_a_shell_command(self):
        qml = (ROOT / "Panel.qml").read_text()
        for forbidden in ("bar.run(", "bash", '"sh"', '"-c"'):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, qml)
        self.assertIn('command: [root.pluginLauncher, "status"]', qml)
        self.assertIn('root.manifest.__sourceDir', qml)
        self.assertIn('"Install OmaSheets"', qml)
        self.assertIn('"review-current"', qml)
        self.assertIn('"open-current"', qml)
        self.assertIn('"window-current"', qml)
        self.assertIn("Open in OmaSheets", qml)
        self.assertIn("Text.PlainText", qml)

    def test_widget_uses_provider_neutral_agent_session_entry(self):
        qml = (ROOT / "Panel.qml").read_text()
        self.assertIn('text: "Ask Agent"', qml)
        self.assertIn('"agent-session"', qml)
        self.assertNotIn("Ask Codex", qml)


if __name__ == "__main__":
    unittest.main()
