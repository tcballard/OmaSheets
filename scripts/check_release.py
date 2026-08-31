#!/usr/bin/env python3
"""Fast release-contract checks using only the Python standard library."""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    project = tomllib.loads((ROOT / "pyproject.toml").read_text())["project"]
    manifest = json.loads((ROOT / "manifest.json").read_text())
    plugin_manifest = json.loads((ROOT / "plugins/omasheets/.codex-plugin/plugin.json").read_text())
    package = (ROOT / "src/omasheets/__init__.py").read_text()
    matched = re.search(r'^__version__\s*=\s*["\']([^"\']+)["\']', package, re.MULTILINE)
    assert matched, "package version is missing"
    versions = {project["version"], manifest["version"], plugin_manifest["version"], matched.group(1)}
    assert len(versions) == 1, f"release versions disagree: {sorted(versions)}"
    assert manifest["id"] == "io.github.tcballard.omasheets"
    for required in (
        "README.md", "INSTALL.md", "docs/ACCEPTANCE.md", "docs/SECURITY.md",
        "docs/AGENT_PROTOCOL.md", "docs/AGENT_WORKFLOWS.md",
        "bin/omasheets-plugin", "scripts/install.py",
        "native/libreofficekit/CMakeLists.txt", "plugins/omasheets/.codex-plugin/plugin.json",
    ):
        assert (ROOT / required).is_file(), f"missing release document: {required}"
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    assert "b686ed892d9c3020c3336203f6d34cc75b544e2b" in workflow, "Omarchy validator pin drifted"
    assert "Arch production install and native acceptance" in workflow
    assert "Exercise the installed agentic workbook loop" in workflow
    assert "omasheets://agent" in (ROOT / "README.md").read_text()
    print(f"release contract ok: v{versions.pop()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
