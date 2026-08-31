from pathlib import Path
import unittest


class NativeAgentEntryTests(unittest.TestCase):
    def test_native_entry_uses_fixed_product_prompt_and_no_shell(self):
        source = (Path(__file__).parents[1] / "native/libreofficekit/window.cpp").read_text()
        function = source.split("void on_ask_agent", 1)[1].split("\n}\n", 1)[0]
        self.assertIn('g_find_program_in_path("codex")', function)
        self.assertIn("omasheets://agent", function)
        self.assertIn("Never publish workbook bytes", function)
        self.assertIn("g_spawn_async", function)
        self.assertNotIn("system(", function)
        self.assertNotIn("/bin/sh", function)


if __name__ == "__main__":
    unittest.main()
