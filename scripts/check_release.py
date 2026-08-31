#!/usr/bin/env python3
"""Fast release-contract checks using only the Python standard library."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from omasheets.native_bundle import require_exact_version_tag  # noqa: E402


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--require-exact-version-tag",
        action="store_true",
        help="also require HEAD to be exactly tagged v<product-version>",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
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
        "docs/AGENT_PROTOCOL.md", "docs/AGENT_WORKFLOWS.md", "docs/PERFORMANCE.md",
        "bin/omasheets-plugin", "scripts/install.py", "scripts/build_native_bundle.py",
        "scripts/performance.py", "src/omasheets/performance.py",
        "src/omasheets/native_bundle.py", ".github/workflows/release.yml",
        "native/libreofficekit/CMakeLists.txt", "plugins/omasheets/.codex-plugin/plugin.json",
    ):
        assert (ROOT / required).is_file(), f"missing release document: {required}"
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    assert "b686ed892d9c3020c3336203f6d34cc75b544e2b" in workflow, "Omarchy validator pin drifted"
    assert "Compiler-free Arch install and native acceptance" in workflow
    assert "archlinux:base\n" in workflow
    assert "libreoffice-fresh-sdk" in workflow
    install_job = workflow.split("  production-install:", 1)[1]
    assert "cmake" not in install_job.split("      - name: Install through", 1)[0]
    assert "OMASHEETS_NATIVE_BUNDLE_PATH" in install_job
    assert "Exercise the installed agentic workbook loop" in workflow
    assert '"agent-session", "call", "query_workbook"' in workflow
    assert "omasheets://session" in (ROOT / "README.md").read_text()
    assert "Ask Agent" in (ROOT / "README.md").read_text()
    assert "omarchy agent prompt" in (ROOT / "README.md").read_text()
    release_workflow = (ROOT / ".github/workflows/release.yml").read_text()
    release_tag_gate = "python scripts/check_release.py --require-exact-version-tag"
    native_bundle_build = "python scripts/build_native_bundle.py"
    assert release_tag_gate in release_workflow
    assert release_workflow.index(release_tag_gate) < release_workflow.index(native_bundle_build)
    version = versions.pop()
    print(f"source tree contract ok: v{version}")
    if arguments.require_exact_version_tag:
        try:
            require_exact_version_tag(ROOT, version)
        except RuntimeError as error:
            print(f"exact version tag contract failed: {error}", file=sys.stderr)
            return 1
        print(f"exact version tag contract ok: v{version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
