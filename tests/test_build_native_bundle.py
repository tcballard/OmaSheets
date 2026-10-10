import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from omasheets import __version__
from omasheets.installation import source_identity
from omasheets.native_bundle import NATIVE_EXECUTABLES, install_native_bundle, normalized_architecture, platform_id
from scripts import build_inputs, build_native_bundle


ROOT = Path(__file__).resolve().parents[1]


class ReproducibleArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def stage(self, name: str, executable_text: str) -> tuple[Path, list[tuple[str, Path, int]]]:
        stage = self.root / name
        (stage / "bin").mkdir(parents=True)
        source = source_identity(ROOT)
        payloads = {}
        for binary in NATIVE_EXECUTABLES:
            path = stage / "bin" / binary
            path.write_text(
                "#!/bin/sh\n"
                f"printf '%s\\n' '{{\"source_commit\":\"{source['commit']}\","
                f"\"source_sha256\":\"{source['sha256']}\"}}'\n" + executable_text
            )
            payloads[f"bin/{binary}"] = path
        manifest = {
            "schema": 1, "version": __version__, "platform": platform_id(),
            "architecture": normalized_architecture(), "source": source,
            "build": {"schema": 1, "image": "archlinux:base-devel@sha256:" + "0" * 64},
            "files": {relative: hashlib.sha256(path.read_bytes()).hexdigest() for relative, path in payloads.items()},
        }
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        members = [("manifest.json", stage / "manifest.json", 0o644)]
        members += [(relative, path, 0o755) for relative, path in payloads.items()]
        return stage, members

    def test_identical_inputs_produce_identical_archives_regardless_of_time_and_order(self):
        _, members = self.stage("one", "# same\n")
        first = self.root / "first.tar.gz"
        second = self.root / "second.tar.gz"
        build_native_bundle.write_reproducible_archive(first, members, 1_700_000_000)
        for path, _, _ in [(member[1], 0, 0) for member in members]:
            os.utime(path, (1, 1))
        build_native_bundle.write_reproducible_archive(second, list(reversed(members)), 1_700_000_000)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        with gzip.open(first, "rb") as compressed:
            raw = compressed.read()
        with tarfile.open(fileobj=io.BytesIO(raw)) as bundle:
            entries = bundle.getmembers()
        self.assertEqual(
            [entry.name for entry in entries],
            sorted([*(f"bin/{name}" for name in NATIVE_EXECUTABLES), "manifest.json"]),
        )
        for entry in entries:
            self.assertEqual((entry.uid, entry.gid, entry.uname, entry.gname, entry.mtime), (0, 0, "", "", 1_700_000_000))
        self.assertEqual({entry.mode for entry in entries}, {0o644, 0o755})
        self.assertEqual(first.read_bytes()[4:8], b"\0\0\0\0", "gzip mtime must be zero")

    def test_different_epoch_or_content_changes_the_digest(self):
        _, members = self.stage("one", "# same\n")
        _, changed = self.stage("two", "# changed\n")
        base = self.root / "base.tar.gz"
        epoch = self.root / "epoch.tar.gz"
        content = self.root / "content.tar.gz"
        build_native_bundle.write_reproducible_archive(base, members, 1)
        build_native_bundle.write_reproducible_archive(epoch, members, 2)
        build_native_bundle.write_reproducible_archive(content, changed, 1)
        digests = {hashlib.sha256(path.read_bytes()).hexdigest() for path in (base, epoch, content)}
        self.assertEqual(len(digests), 3)

    def test_reproducible_archive_installs_through_the_bundle_validator(self):
        _, members = self.stage("one", "")
        archive = self.root / "bundle.tar.gz"
        build_native_bundle.write_reproducible_archive(archive, members, 1_700_000_000)
        destination = self.root / "app"
        manifest = install_native_bundle(archive, destination, version=__version__, source=source_identity(ROOT))
        self.assertEqual(manifest["build"]["image"], "archlinux:base-devel@sha256:" + "0" * 64)
        self.assertEqual(json.loads((destination / "native-bundle.json").read_text())["build"], manifest["build"])
        for binary in NATIVE_EXECUTABLES:
            self.assertEqual((destination / "bin" / binary).stat().st_mode & 0o777, 0o755)

    def test_bundle_distributes_only_owned_runtime_executables(self):
        _, members = self.stage("native-kit", "")
        archive = self.root / "native-kit.tar.gz"
        build_native_bundle.write_reproducible_archive(archive, members, 1_700_000_000)
        destination = self.root / "native-kit-app"
        manifest = install_native_bundle(archive, destination, version=__version__, source=source_identity(ROOT))
        self.assertIn("bin/omasheets-kit", manifest["files"])
        self.assertEqual(set(NATIVE_EXECUTABLES), {
            "omasheets-kit", "omasheets-service", "omasheets-grid", "omasheets-setup",
        })
        for binary in NATIVE_EXECUTABLES:
            self.assertTrue((destination / "bin" / binary).is_file())
        for binary in ("omasheets-window", "omasheets-lok-render"):
            self.assertFalse((destination / "bin" / binary).exists())

    def test_legacy_office_payload_is_rejected_before_publishing_files(self):
        _, members = self.stage("legacy-payload", "")
        legacy = self.root / "omasheets-window"
        legacy.write_bytes(b"legacy office window")
        for binary in ("omasheets-window", "omasheets-lok-render"):
            archive = self.root / f"{binary}.tar.gz"
            build_native_bundle.write_reproducible_archive(
                archive, members + [(f"bin/{binary}", legacy, 0o755)], 1,
            )
            destination = self.root / f"{binary}-app"
            with self.assertRaisesRegex(RuntimeError, "unexpected file set"):
                install_native_bundle(archive, destination, version=__version__, source=source_identity(ROOT))
            self.assertFalse(destination.exists())

    def test_native_bundle_refuses_a_missing_native_kit_before_installing_anything(self):
        _, members = self.stage("missing-kit", "")
        archive = self.root / "missing-kit.tar.gz"
        build_native_bundle.write_reproducible_archive(
            archive, [member for member in members if member[0] != "bin/omasheets-kit"], 1,
        )
        destination = self.root / "missing-kit-app"
        with self.assertRaisesRegex(RuntimeError, "unexpected file set"):
            install_native_bundle(archive, destination, version=__version__, source=source_identity(ROOT))
        self.assertFalse(destination.exists())

    def test_source_date_epoch_prefers_the_environment_then_the_commit_time(self):
        with patch.dict(os.environ, {"SOURCE_DATE_EPOCH": "12345"}):
            self.assertEqual(build_native_bundle.source_date_epoch(), 12345)
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("SOURCE_DATE_EPOCH", None)
            self.assertGreater(build_native_bundle.source_date_epoch(), 1_600_000_000)


class BuildInputsTests(unittest.TestCase):
    def test_default_build_packages_do_not_require_foreign_office_components(self):
        self.assertIn("rust", build_inputs.DEFAULT_PACKAGES)
        self.assertIn("qt6-declarative", build_inputs.DEFAULT_PACKAGES)
        self.assertFalse(any("libreoffice" in name or "uno" in name for name in build_inputs.DEFAULT_PACKAGES))

    def test_record_names_every_pinned_input(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(
            build_inputs, "installed_versions", return_value={"gcc": "15.2.1+r1-1", "cmake": "4.1.1-1"},
        ), patch.dict(os.environ, {
            "GITHUB_REPOSITORY": "tcballard/OmaSheets", "GITHUB_WORKFLOW_REF": "tcballard/OmaSheets/.github/workflows/release.yml@refs/tags/v9",
            "GITHUB_SHA": "a" * 40, "GITHUB_RUN_ID": "1", "GITHUB_RUN_ATTEMPT": "1",
        }):
            output = Path(temporary) / "build-inputs.json"
            code = build_inputs.main([
                "--image", "archlinux:base-devel@sha256:" + "f" * 64, "--snapshot", "2026/09/01",
                "--source-date-epoch", "1700000000", "--cxxflags=-ffile-prefix-map=/work=/omasheets",
                "--package", "gcc", "--package", "cmake", "--output", str(output),
            ])
            self.assertEqual(code, 0)
            record = json.loads(output.read_text())
        self.assertEqual(record["schema"], 1)
        self.assertEqual(record["package_snapshot"], "https://archive.archlinux.org/repos/2026/09/01/")
        self.assertEqual(record["packages"], {"gcc": "15.2.1+r1-1", "cmake": "4.1.1-1"})
        self.assertEqual(record["source_date_epoch"], 1700000000)
        self.assertEqual(record["workflow"]["ref"], "tcballard/OmaSheets/.github/workflows/release.yml@refs/tags/v9")
        self.assertEqual(record["workflow"]["sha"], "a" * 40)

    def test_unpinned_image_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary, self.assertRaises(SystemExit):
            build_inputs.main([
                "--image", "archlinux:base-devel", "--snapshot", "2026/09/01",
                "--source-date-epoch", "1", "--output", str(Path(temporary) / "x.json"),
            ])

    def test_missing_packages_are_reported(self):
        completed = type("Completed", (), {"stdout": "gcc 15.2.1-1\n"})()
        with patch.object(build_inputs.subprocess, "run", return_value=completed):
            with self.assertRaisesRegex(RuntimeError, "not installed: cmake"):
                build_inputs.installed_versions(("gcc", "cmake"))
            self.assertEqual(build_inputs.installed_versions(("gcc",)), {"gcc": "15.2.1-1"})


class NativeBuildTests(unittest.TestCase):
    def test_bundle_build_uses_only_owned_rust_build_targets(self):
        identity = {"commit": "a" * 40, "sha256": "b" * 64}
        commands = []

        def cargo_build(command, **kwargs):
            commands.append(command)
            self.assertEqual(command[0], "cargo")
            target = Path(kwargs["env"]["CARGO_TARGET_DIR"]) / "release"
            target.mkdir(parents=True, exist_ok=True)
            if "--manifest-path" not in command:
                binaries = ("omasheets-service", "omasheets-kit")
            elif "qt-grid" in command[-1]:
                binaries = ("omasheets-grid",)
            else:
                binaries = ("omasheets-setup",)
            for binary in binaries:
                (target / binary).write_bytes(b"owned executable")

        with tempfile.TemporaryDirectory() as temporary, patch.object(
            build_native_bundle, "source_identity", return_value=identity,
        ), patch.object(build_native_bundle, "source_date_epoch", return_value=1), patch.object(
            build_native_bundle.subprocess, "run", side_effect=cargo_build,
        ), patch.object(build_native_bundle, "verify_owned_runtime") as audit:
            directory = Path(temporary)
            output = directory / "output"
            self.assertEqual(build_native_bundle.main([
                "--output", str(output), "--build-dir", str(directory / "build"),
            ]), 0)
            audit.assert_called_once()
            archive = output / build_native_bundle.asset_name(__version__)
            with tarfile.open(archive) as bundle:
                self.assertEqual(set(bundle.getnames()), {
                    "manifest.json", *(f"bin/{name}" for name in NATIVE_EXECUTABLES),
                })
                manifest = json.load(bundle.extractfile("manifest.json"))
            self.assertEqual(manifest["source"], identity)
            self.assertEqual(manifest["build_contract"], "Cargo.toml")
            self.assertIn("crates/omasheets-kit/Cargo.toml", manifest["rust_build_contract"])
            self.assertIn("native/setup/Cargo.toml", manifest["rust_build_contract"])
        self.assertEqual(len(commands), 3)
        self.assertFalse(any("libreoffice" in str(command).lower() or "cmake" in command for command in commands))

    def test_linkage_audit_rejects_office_and_missing_libraries(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "omasheets-kit").write_bytes(b"\x7fELFowned")
            for output in (
                "libmergedlo.so => /usr/lib/libreoffice/program/libmergedlo.so\n",
                "libuno_sal.so.3 => /usr/lib/libuno_sal.so.3\n",
                "libQt6Quick.so.6 => not found\n",
            ):
                with self.subTest(output=output), patch.object(
                    build_native_bundle.subprocess, "run",
                    return_value=subprocess.CompletedProcess([], 0, output, ""),
                ), self.assertRaisesRegex(RuntimeError, "depends on LibreOffice/UNO|unresolved runtime libraries"):
                    build_native_bundle.verify_owned_runtime(directory, ("omasheets-kit",))

    def test_linkage_audit_accepts_resolved_owned_runtime_and_static_elf(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            executable = directory / "omasheets-kit"
            executable.write_bytes(b"\x7fELFowned")
            for code, output in (
                (0, "libQt6Quick.so.6 => /usr/lib/libQt6Quick.so.6\nlibc.so.6 => /usr/lib/libc.so.6\n"),
                (1, "not a dynamic executable\n"),
            ):
                with patch.object(
                    build_native_bundle.subprocess, "run",
                    return_value=subprocess.CompletedProcess([], code, output, ""),
                ):
                    build_native_bundle.verify_owned_runtime(directory, ("omasheets-kit",))
            executable.write_bytes(b"#!/bin/sh\nexit 0\n")
            with self.assertRaisesRegex(RuntimeError, "not ELF"):
                build_native_bundle.verify_owned_runtime(directory, ("omasheets-kit",))


if __name__ == "__main__":
    unittest.main()
