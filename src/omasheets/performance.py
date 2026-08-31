"""Dependency-free performance evidence and deterministic workbook fixtures.

The benchmark path deliberately stays outside the product runtime.  It records
what Linux exposes through ``/proc`` and never substitutes an estimated PSS or
USS when the kernel does not make those metrics readable.
"""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
from typing import Iterable, Sequence


RESULT_SCHEMA = "OMASHEETS_PERFORMANCE_V1"
FIXTURE_SCHEMA = "OMASHEETS_PERFORMANCE_FIXTURES_V1"
MAX_JSON_BYTES = 1_048_576
MAX_RETAINED_SAMPLES = 2_048
_NAME = re.compile(r"^[a-z0-9][a-z0-9._-]{0,79}$")


@dataclass(frozen=True)
class MemorySample:
    """One process-group memory observation."""

    at_seconds: float
    process_count: int
    readable_processes: int
    rss_bytes: int | None
    pss_bytes: int | None
    uss_bytes: int | None
    source: str

    def as_dict(self) -> dict:
        return {
            "at_seconds": round(self.at_seconds, 6),
            "process_count": self.process_count,
            "readable_processes": self.readable_processes,
            "rss_bytes": self.rss_bytes,
            "pss_bytes": self.pss_bytes,
            "uss_bytes": self.uss_bytes,
            "source": self.source,
        }


class ProcProcessGroupSampler:
    """Sum memory for an isolated command group and all of its descendants.

    Descendants are retained even if they create another process group or
    session.  That matters for Bubblewrap's ``--new-session`` boundary: it
    must not make the Calc worker disappear from an OmaSheets measurement.
    """

    def __init__(self, proc_root: Path = Path("/proc")) -> None:
        self.proc_root = proc_root

    @staticmethod
    def _process_identity(stat: str) -> tuple[int, int] | None:
        # The command name is parenthesised and may itself contain spaces or ')'.
        closing = stat.rfind(")")
        if closing < 0:
            return None
        fields = stat[closing + 1 :].split()
        if len(fields) < 3:
            return None
        try:
            return int(fields[1]), int(fields[2])
        except ValueError:
            return None

    @classmethod
    def _process_group(cls, stat: str) -> int | None:
        identity = cls._process_identity(stat)
        return None if identity is None else identity[1]

    @staticmethod
    def _kilobytes(path: Path, wanted: set[str]) -> dict[str, int]:
        values: dict[str, int] = {}
        with path.open(encoding="utf-8", errors="replace") as source:
            for line in source:
                key, separator, remainder = line.partition(":")
                if not separator or key not in wanted:
                    continue
                fields = remainder.split()
                if fields:
                    values[key] = int(fields[0])
        return values

    def _memory(self, pid: int) -> tuple[int | None, int | None, int | None, str]:
        process = self.proc_root / str(pid)
        try:
            values = self._kilobytes(
                process / "smaps_rollup",
                {"Rss", "Pss", "Private_Clean", "Private_Dirty", "Private_Hugetlb"},
            )
            if "Rss" in values and "Pss" in values:
                private = None
                if "Private_Clean" in values and "Private_Dirty" in values:
                    private = sum(
                        values.get(key, 0)
                        for key in ("Private_Clean", "Private_Dirty", "Private_Hugetlb")
                    ) * 1024
                return values["Rss"] * 1024, values["Pss"] * 1024, private, "smaps_rollup"
        except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError, OSError):
            pass
        try:
            values = self._kilobytes(process / "status", {"VmRSS"})
            if "VmRSS" in values:
                return values["VmRSS"] * 1024, None, None, "status"
        except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError, OSError):
            pass
        return None, None, None, "unreadable"

    def _members(self, process_group: int) -> list[int]:
        if not self.proc_root.is_dir():
            return []
        processes: dict[int, tuple[int, int]] = {}
        try:
            entries = self.proc_root.iterdir()
        except OSError:
            return []
        for entry in entries:
            if not entry.name.isdecimal():
                continue
            try:
                stat = (entry / "stat").read_text(encoding="utf-8", errors="replace")
            except (FileNotFoundError, PermissionError, ProcessLookupError, OSError):
                continue
            identity = self._process_identity(stat)
            if identity is not None:
                processes[int(entry.name)] = identity

        # The measured process is a session leader with pid == process_group.
        # Start with that complete group, then follow parent links so a child
        # that calls setsid() is still observed while its parent is alive.
        members = {
            pid for pid, (_parent, group) in processes.items()
            if group == process_group
        }
        while True:
            descendants = {
                pid for pid, (parent, _group) in processes.items()
                if parent in members
            }
            expanded = members | descendants
            if expanded == members:
                return sorted(members)
            members = expanded

    def sample(self, process_group: int, at_seconds: float) -> MemorySample:
        members = self._members(process_group)
        rows = [self._memory(pid) for pid in members]
        readable = sum(row[0] is not None for row in rows)
        rss = sum(row[0] for row in rows if row[0] is not None) if readable == len(members) and members else None
        pss = sum(row[1] for row in rows if row[1] is not None) if rows and all(row[1] is not None for row in rows) else None
        uss = sum(row[2] for row in rows if row[2] is not None) if rows and all(row[2] is not None for row in rows) else None
        sources = {row[3] for row in rows}
        if not members:
            source = "proc_unavailable" if not self.proc_root.is_dir() else "no_processes"
        elif sources == {"smaps_rollup"}:
            source = "smaps_rollup"
        elif sources == {"status"}:
            source = "status"
        elif sources <= {"smaps_rollup", "status"}:
            source = "mixed"
        else:
            source = "incomplete"
        return MemorySample(at_seconds, len(members), readable, rss, pss, uss, source)


class _SampleAccumulator:
    """Retain bounded observations while computing peaks across every sample."""

    def __init__(self, maximum: int) -> None:
        if not 2 <= maximum <= MAX_RETAINED_SAMPLES:
            raise ValueError(f"max_samples must be between 2 and {MAX_RETAINED_SAMPLES}")
        self.maximum = maximum
        self.observed = 0
        self.samples: list[MemorySample] = []
        self.peaks: dict[str, int | None] = {
            "rss_bytes": None,
            "pss_bytes": None,
            "uss_bytes": None,
            "process_count": None,
        }
        self.sources: Counter[str] = Counter()

    def add(self, sample: MemorySample) -> None:
        self.observed += 1
        self.sources[sample.source] += 1
        for field in ("rss_bytes", "pss_bytes", "uss_bytes", "process_count"):
            value = getattr(sample, field)
            current = self.peaks[field]
            if value is not None and (current is None or value > current):
                self.peaks[field] = value
        if len(self.samples) < self.maximum:
            self.samples.append(sample)
        else:
            # Preserve the beginning and always retain the newest observation.
            self.samples[-1] = sample

    def report(self) -> dict:
        return {
            "measurement": "sum of the isolated command process group and descendant processes",
            "available": any(self.peaks[key] is not None for key in ("rss_bytes", "pss_bytes", "uss_bytes")),
            "peak_rss_bytes": self.peaks["rss_bytes"],
            "peak_pss_bytes": self.peaks["pss_bytes"],
            "peak_uss_bytes": self.peaks["uss_bytes"],
            "peak_process_count": self.peaks["process_count"],
            "source_sample_counts": dict(sorted(self.sources.items())),
        }


def _bounded_command(command: Sequence[str], include_command: bool) -> dict:
    if not command or any(not isinstance(part, str) or not part or "\0" in part for part in command):
        raise ValueError("command must contain non-empty strings without NUL bytes")
    encoded = "\0".join(command).encode("utf-8", errors="surrogateescape")
    if len(command) > 128 or len(encoded) > 32_768:
        raise ValueError("command exceeds the bounded argument contract")
    result = {
        "executable": Path(command[0]).name[:256],
        "argument_count": len(command) - 1,
        "sha256": hashlib.sha256(encoded).hexdigest(),
        "argv_recorded": include_command,
    }
    if include_command:
        result["argv"] = list(command)
    return result


def _terminate(process: subprocess.Popen, grace_seconds: float = 1.0) -> None:
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        pass
    try:
        process.wait(timeout=grace_seconds)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass
    process.wait()


def measure_command(
    name: str,
    command: Sequence[str],
    *,
    interval_seconds: float = 0.1,
    timeout_seconds: float | None = None,
    max_samples: int = 512,
    include_command: bool = False,
    sampler: ProcProcessGroupSampler | None = None,
) -> dict:
    """Run a foreground command and return bounded wall/memory evidence."""

    if not _NAME.fullmatch(name):
        raise ValueError("benchmark name must be a bounded lowercase identifier")
    if not 0.01 <= interval_seconds <= 60:
        raise ValueError("interval_seconds must be between 0.01 and 60")
    if timeout_seconds is not None and not 0.05 <= timeout_seconds <= 86_400:
        raise ValueError("timeout_seconds must be between 0.05 and 86400")
    command_record = _bounded_command(command, include_command)
    observations = _SampleAccumulator(max_samples)
    memory_sampler = sampler or ProcProcessGroupSampler()
    started_at = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    started = time.monotonic()
    process = subprocess.Popen(
        list(command),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    timed_out = False
    while True:
        elapsed = time.monotonic() - started
        observations.add(memory_sampler.sample(process.pid, elapsed))
        if process.poll() is not None:
            break
        if timeout_seconds is not None and elapsed >= timeout_seconds:
            timed_out = True
            _terminate(process)
            break
        sleep_for = interval_seconds
        if timeout_seconds is not None:
            sleep_for = min(sleep_for, max(0.001, timeout_seconds - elapsed))
        time.sleep(sleep_for)
    exit_code = process.wait()
    duration = time.monotonic() - started
    result = {
        "schema": RESULT_SCHEMA,
        "name": name,
        "started_at_utc": started_at,
        "wall_seconds": round(duration, 6),
        "exit_code": exit_code,
        "timed_out": timed_out,
        "command": command_record,
        "sampling": {
            "interval_seconds": interval_seconds,
            "observed_samples": observations.observed,
            "retained_samples": len(observations.samples),
            "max_retained_samples": max_samples,
        },
        "memory": observations.report(),
        "samples": [sample.as_dict() for sample in observations.samples],
        "notes": [
            "RSS double-counts shared pages; PSS apportions them; USS contains private pages.",
            "Null PSS or USS means /proc/smaps_rollup was unavailable for at least one process.",
        ],
    }
    bounded_json(result)
    return result


@dataclass(frozen=True)
class FixtureSpec:
    """Exact dimensions and population pattern for one deterministic FODS file."""

    name: str
    kind: str
    data_rows: int
    columns: int
    row_stride: int = 1
    column_stride: int = 1

    def validate(self) -> None:
        if not _NAME.fullmatch(self.name):
            raise ValueError("fixture name must be a bounded lowercase identifier")
        if self.kind not in {"dense", "sparse", "formula"}:
            raise ValueError("fixture kind must be dense, sparse, or formula")
        if not 1 <= self.data_rows <= 2_000_000 or not 1 <= self.columns <= 1_024:
            raise ValueError("fixture dimensions exceed the bounded generator contract")
        if self.data_rows * self.columns > 100_000_000:
            raise ValueError("fixture exceeds 100 million logical data cells")
        if self.kind == "formula" and self.columns < 3:
            raise ValueError("formula fixtures require at least three columns")
        if self.row_stride < 1 or self.column_stride < 1:
            raise ValueError("fixture strides must be positive")

    @staticmethod
    def _selected(size: int, stride: int) -> tuple[int, ...]:
        selected = list(range(1, size + 1, stride))
        if selected[-1] != size:
            selected.append(size)
        return tuple(selected)

    def description(self) -> dict:
        self.validate()
        logical = self.data_rows * self.columns
        if self.kind == "dense":
            values, formulas = logical, 0
        elif self.kind == "formula":
            values = self.data_rows * 2
            formulas = self.data_rows * (self.columns - 2)
        else:
            values = len(self._selected(self.data_rows, self.row_stride)) * len(
                self._selected(self.columns, self.column_stride)
            )
            formulas = 0
        return {
            "name": self.name,
            "kind": self.kind,
            "format": "fods",
            "data_rows": self.data_rows,
            "columns": self.columns,
            "logical_data_cells": logical,
            "header_cells": self.columns,
            "used_range_cells": logical + self.columns,
            "value_cells": values,
            "formula_cells": formulas,
            "populated_data_cells": values + formulas,
            "data_density": round((values + formulas) / logical, 10),
            "row_stride": self.row_stride if self.kind == "sparse" else None,
            "column_stride": self.column_stride if self.kind == "sparse" else None,
        }


def fixture_specs(profile: str = "standard") -> tuple[FixtureSpec, ...]:
    if profile == "smoke":
        return (
            FixtureSpec("dense-smoke", "dense", 100, 20),
            FixtureSpec("sparse-smoke", "sparse", 1_000, 20, 50, 5),
            FixtureSpec("formula-smoke", "formula", 100, 10),
        )
    if profile == "standard":
        return (
            FixtureSpec("dense-100k-x50", "dense", 100_000, 50),
            FixtureSpec("sparse-1m-x50", "sparse", 1_000_000, 50, 100, 10),
            FixtureSpec("formula-100k-x10", "formula", 100_000, 10),
        )
    if profile == "ci":
        # Each used range, including its header, remains beneath CalcEngine's
        # 250,000 inspected-cell limit.  The formula case also stays below the
        # separate 20,000-formula limit so every shape can traverse the agent
        # analysis path in hosted acceptance when needed.
        return (
            FixtureSpec("dense-ci", "dense", 12_000, 20),
            FixtureSpec("sparse-ci", "sparse", 12_000, 20, 100, 5),
            FixtureSpec("formula-ci", "formula", 2_000, 10),
        )
    raise ValueError("fixture profile must be smoke, ci, or standard")


_FODS_HEADER = """<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:mimetype="application/vnd.oasis.opendocument.spreadsheet" office:version="1.3">
  <office:body>
    <office:spreadsheet>
"""
_FODS_FOOTER = """    </office:spreadsheet>
  </office:body>
</office:document>
"""


def _column_name(number: int) -> str:
    result = ""
    while number:
        number, remainder = divmod(number - 1, 26)
        result = chr(65 + remainder) + result
    return result


def _string_cell(value: str) -> str:
    return f'<table:table-cell office:value-type="string"><text:p>{value}</text:p></table:table-cell>'


def _number_cell(value: int) -> str:
    return f'<table:table-cell office:value-type="float" office:value="{value}"><text:p>{value}</text:p></table:table-cell>'


def _formula_cell(formula: str, cached: int) -> str:
    return (
        f'<table:table-cell table:formula="{formula}" office:value-type="float" '
        f'office:value="{cached}"><text:p>{cached}</text:p></table:table-cell>'
    )


def _headers(spec: FixtureSpec) -> list[str]:
    if spec.kind != "formula":
        return [f"column_{column:04d}" for column in range(1, spec.columns + 1)]
    return ["input_row", "input_value"] + [
        f"formula_{column:04d}" for column in range(1, spec.columns - 1)
    ]


def _write_dense(destination, spec: FixtureSpec) -> None:
    for row in range(1, spec.data_rows + 1):
        cells = [
            _number_cell((row * 104_729 + column * 13_007) % 1_000_000)
            for column in range(1, spec.columns + 1)
        ]
        destination.write("      <table:table-row>" + "".join(cells) + "</table:table-row>\n")


def _write_sparse(destination, spec: FixtureSpec) -> None:
    selected_rows = spec._selected(spec.data_rows, spec.row_stride)
    selected_columns = spec._selected(spec.columns, spec.column_stride)
    next_row = 1
    for row in selected_rows:
        gap = row - next_row
        if gap:
            destination.write(f'      <table:table-row table:number-rows-repeated="{gap}"/>\n')
        cells: list[str] = []
        next_column = 1
        for column in selected_columns:
            column_gap = column - next_column
            if column_gap:
                cells.append(f'<table:table-cell table:number-columns-repeated="{column_gap}"/>')
            cells.append(_number_cell(row * 1_000_003 + column))
            next_column = column + 1
        destination.write("      <table:table-row>" + "".join(cells) + "</table:table-row>\n")
        next_row = row + 1


def _write_formulas(destination, spec: FixtureSpec) -> None:
    for row in range(1, spec.data_rows + 1):
        sheet_row = row + 1
        input_value = (row * 17) % 1_000
        cached = row + input_value
        cells = [_number_cell(row), _number_cell(input_value)]
        for column in range(3, spec.columns + 1):
            if column == 3:
                previous = "A"
            else:
                previous = _column_name(column - 1)
                cached += input_value
            formula = f"of:=[.{previous}{sheet_row}]+[.B{sheet_row}]"
            cells.append(_formula_cell(formula, cached))
        destination.write("      <table:table-row>" + "".join(cells) + "</table:table-row>\n")


def generate_fixture(destination: Path, spec: FixtureSpec) -> dict:
    """Create one deterministic, no-clobber FODS fixture and return its facts."""

    spec.validate()
    destination = Path(destination)
    if destination.suffix != ".fods":
        raise ValueError("fixture destination must use the .fods extension")
    if destination.exists():
        raise FileExistsError(f"performance fixture already exists: {destination.name}")
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            newline="\n",
            dir=destination.parent,
            prefix=f".{destination.name}.",
            suffix=".tmp",
            delete=False,
        ) as output:
            temporary_path = Path(output.name)
            output.write(_FODS_HEADER)
            output.write(f'      <table:table table:name="{spec.name}">\n')
            output.write("      <table:table-row>" + "".join(_string_cell(value) for value in _headers(spec)) + "</table:table-row>\n")
            if spec.kind == "dense":
                _write_dense(output, spec)
            elif spec.kind == "sparse":
                _write_sparse(output, spec)
            else:
                _write_formulas(output, spec)
            output.write("      </table:table>\n")
            output.write(_FODS_FOOTER)
        # A same-directory hard link is an atomic, no-clobber publication. It
        # also means a disk-full generator never exposes a partial fixture at
        # the requested path.
        os.link(temporary_path, destination)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
    digest = hashlib.sha256()
    with destination.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return {
        **spec.description(),
        "file": destination.name,
        "bytes": destination.stat().st_size,
        "sha256": digest.hexdigest(),
    }


def fixture_manifest(profile: str = "standard") -> dict:
    return {
        "schema": FIXTURE_SCHEMA,
        "profile": profile,
        "fixtures": [spec.description() for spec in fixture_specs(profile)],
    }


def generate_fixture_suite(directory: Path, profile: str = "standard") -> dict:
    """Materialise a profile after a complete no-clobber preflight."""

    specs = fixture_specs(profile)
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    destinations = [directory / f"{spec.name}.fods" for spec in specs]
    manifest_path = directory / f"{profile}-manifest.json"
    existing = [path.name for path in (*destinations, manifest_path) if path.exists()]
    if existing:
        raise FileExistsError(f"performance fixture outputs already exist: {', '.join(sorted(existing))}")
    fixtures = [generate_fixture(path, spec) for path, spec in zip(destinations, specs)]
    manifest = {"schema": FIXTURE_SCHEMA, "profile": profile, "fixtures": fixtures}
    write_bounded_json(manifest_path, manifest)
    return manifest


def bounded_json(payload: dict, maximum_bytes: int = MAX_JSON_BYTES) -> str:
    text = json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=True) + "\n"
    if len(text.encode("utf-8")) > maximum_bytes:
        raise ValueError(f"JSON evidence exceeds the {maximum_bytes}-byte bound")
    return text


def write_bounded_json(destination: Path, payload: dict) -> None:
    text = bounded_json(payload)
    with Path(destination).open("x", encoding="utf-8", newline="\n") as output:
        output.write(text)


def descriptions(specs: Iterable[FixtureSpec]) -> list[dict]:
    """Return stable descriptions for caller-supplied fixture specifications."""

    return [spec.description() for spec in specs]
