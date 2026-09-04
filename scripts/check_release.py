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

from omasheets.native_bundle import RELEASE_SIGNING_KEY, require_exact_version_tag  # noqa: E402
from omasheets.release_signing import SignatureError, load_public_key  # noqa: E402

_ACTION_PIN = re.compile(r"^\s*(?:-\s+)?uses:\s*([^@\s]+)@([0-9a-f]{40})\s*#\s*v\S+\s*$")


def check_release_workflow(text: str) -> None:
    """Every release build input must be pinned: actions, image and packages."""
    uses = [line for line in text.splitlines() if re.match(r"^\s*(?:-\s+)?uses:", line)]
    assert uses, "release workflow declares no actions"
    for line in uses:
        assert _ACTION_PIN.match(line), f"release action is not pinned to a full commit SHA: {line.strip()}"
    images = re.findall(r"^\s*image:\s*(\S+)\s*$", text, re.MULTILINE)
    assert images, "release workflow declares no container image"
    for image in images:
        assert re.fullmatch(r"[a-z0-9./_-]+:[A-Za-z0-9._-]+@sha256:[0-9a-f]{64}", image), (
            f"release image is not pinned by digest: {image}"
        )
    assert "archive.archlinux.org/repos/" in text, "release packages are not pinned to an Archive snapshot"
    assert re.search(r"^\s*ARCH_SNAPSHOT:\s*\d{4}/\d{2}/\d{2}\s*$", text, re.MULTILINE), "ARCH_SNAPSHOT is missing"
    assert "pacman -Syyuu" in text, "release package set must converge on the snapshot"
    assert "scripts/build_inputs.py" in text and "--build-inputs" in text, "build inputs are not recorded"
    assert "SOURCE_DATE_EPOCH=" in text and "-ffile-prefix-map=" in text, "build is not reproducible"
    assert "actions/attest-build-provenance@" in text, "bundle provenance is not attested"
    assert "secrets." not in text, "the release workflow must hold no signing secret"


def check_pinned_release_key(require: bool) -> None:
    path = ROOT / RELEASE_SIGNING_KEY
    if not path.is_file():
        assert not require, f"release gate requires the pinned signing key at {RELEASE_SIGNING_KEY}"
        return
    try:
        key = load_public_key(path)
    except SignatureError as error:
        raise AssertionError(f"pinned release signing key is invalid: {error}") from error
    print(f"pinned release signing key ok: {key.key_id_hex}")


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
        "docs/ADR-0003-EVENT-SOURCED-NATIVE-CORE.md", "docs/ROADMAP.md",
        "bin/omasheets-plugin", "scripts/install.py", "scripts/build_native_bundle.py",
        "scripts/performance.py", "src/omasheets/performance.py",
        "src/omasheets/native_bundle.py", ".github/workflows/release.yml",
        "src/omasheets/release_signing.py", "src/omasheets/bounded_process.py",
        "scripts/panel_status.py", "scripts/build_inputs.py",
        "scripts/check_native_service_install.py",
        "scripts/verify_release_signature.py", "docs/RELEASE.md",
        "native/libreofficekit/CMakeLists.txt", "plugins/omasheets/.codex-plugin/plugin.json",
    ):
        assert (ROOT / required).is_file(), f"missing release document: {required}"
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    assert "b686ed892d9c3020c3336203f6d34cc75b544e2b" in workflow, "Omarchy validator pin drifted"
    for line in workflow.splitlines():
        if re.match(r"^\s*(?:-\s+)?uses:", line):
            assert _ACTION_PIN.match(line), f"CI action is not pinned to a full commit SHA: {line.strip()}"
    assert "Compiler-free Arch install and native acceptance" in workflow
    assert "archlinux:base\n" in workflow
    assert "libreoffice-fresh-sdk" in workflow
    install_job = workflow.split("  production-install:", 1)[1]
    assert "cmake" not in install_job.split("      - name: Install through", 1)[0]
    assert "OMASHEETS_NATIVE_BUNDLE_PATH" in install_job
    assert "Exercise the installed agentic workbook loop" in workflow
    assert "Exercise the installed native service workflow" in workflow
    assert '"agent-session", "call", "query_workbook"' in workflow
    assert "omasheets://session" in (ROOT / "README.md").read_text()
    assert "Ask Agent" in (ROOT / "README.md").read_text()
    assert "omarchy agent prompt" in (ROOT / "README.md").read_text()
    release_workflow = (ROOT / ".github/workflows/release.yml").read_text()
    release_tag_gate = "python scripts/check_release.py --require-exact-version-tag"
    native_bundle_build = "python scripts/build_native_bundle.py"
    assert release_tag_gate in release_workflow
    assert release_workflow.index(release_tag_gate) < release_workflow.index(native_bundle_build)
    check_release_workflow(release_workflow)
    for required in (
        "release/signing-key.pub", ".minisig", "minisign", "attest", "docs/RELEASE.md",
    ):
        assert required in (ROOT / "INSTALL.md").read_text() + (ROOT / "docs/SECURITY.md").read_text(), (
            f"trust-root documentation is missing: {required}"
        )
    plugin_helper = (ROOT / "bin/omasheets-plugin").read_text()
    assert "scripts/panel_status.py" in plugin_helper, "panel status is not bounded by the helper"
    assert "status --json" not in plugin_helper, "panel status must not run the launcher unbounded"
    check_pinned_release_key(arguments.require_exact_version_tag)
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
