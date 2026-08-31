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
    package = (ROOT / "src/omasheets/__init__.py").read_text()
    matched = re.search(r'^__version__\s*=\s*["\']([^"\']+)["\']', package, re.MULTILINE)
    assert matched, "package version is missing"
    versions = {project["version"], manifest["version"], matched.group(1)}
    assert len(versions) == 1, f"release versions disagree: {sorted(versions)}"
    assert manifest["id"] == "io.github.tcballard.omasheets"
    for required in ("README.md", "INSTALL.md", "docs/ACCEPTANCE.md", "docs/SECURITY.md"):
        assert (ROOT / required).is_file(), f"missing release document: {required}"
    print(f"release contract ok: v{versions.pop()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
