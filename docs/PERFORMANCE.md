# Performance evidence

OmaSheets does not yet claim a production memory or large-workbook profile.
The dependency-free harness in `scripts/performance.py` establishes a
repeatable baseline without adding a third-party runtime dependency or resident
process. It runs on Linux using only Python's standard library.

## What is measured

`run` starts one foreground command in a new process group and samples that
group plus its descendant processes from `/proc`. Descendants remain in scope
if they create another process group or session, including the Calc worker
behind Bubblewrap's `--new-session`. Every observation records process count
plus:

- RSS, the resident total which double-counts shared pages;
- PSS, the resident total with shared pages apportioned among users; and
- USS, the private clean, dirty, and huge pages unique to those processes.

PSS and USS come from `/proc/<pid>/smaps_rollup`. If it cannot be read for even
one member, those aggregate fields are `null`; the harness never turns RSS into
an estimate. A `VmRSS` fallback can still supply RSS. Reports retain at most
2,048 samples, are capped at 1 MiB, discard command output, and omit argv by
default so a token in an argument is not copied into benchmark evidence.

The measured program must remain in the foreground. Its live descendants are
followed through their parent links even when they change process group; a
daemon which outlives and detaches from the measured parent tree remains
outside the report. Run the native executable directly rather than a launcher
which exits immediately. PSS also depends on what else shares each page during
the sample, so record the machine, kernel, installed LibreOffice build, and
whether the run was cold or warm beside any published result.

```bash
python3 scripts/performance.py run \
  --name native-idle-small \
  --timeout 15 \
  --output /tmp/native-idle-small.json \
  -- ~/.local/share/omasheets/app/bin/omasheets-window \
    --smoke-test /tmp/native-idle-small.png /tmp/small.fods
```

The smoke window exits after capturing its first useful render; run it from a
graphical session (or under Xvfb in CI). The timeout terminates only the
isolated command tree observed by the sampler, including descendants that
create another process group or session. The report records whether termination
completed. A non-zero exit or timeout makes the script fail while still writing
the bounded report.

## Truthful deterministic fixtures

Inspect the exact specifications without allocating workbook data:

```bash
python3 scripts/performance.py specs --profile standard
```

Generate the quick integration set, bounded hosted-CI set, or large standard
set into a new or empty directory:

```bash
python3 scripts/performance.py fixtures --profile smoke --directory /tmp/omasheets-smoke
python3 scripts/performance.py fixtures --profile ci --directory /tmp/omasheets-ci
python3 scripts/performance.py fixtures --profile standard --directory /tmp/omasheets-standard
```

Generation is deterministic and refuses to replace any output. Each manifest
records the file SHA-256, byte size, logical cells, value cells, formula cells,
and actual data density. FODS keeps generation dependency-free and lets the
same source be opened directly by Calc; an engine run must explicitly request
recalculation rather than treating cached formula results as calculation proof.

The standard corpus is deliberately different from the old large-used-range
smoke fixture:

| Fixture | Logical data cells | Values | Formulas | Purpose |
|---|---:|---:|---:|---|
| `dense-100k-x50` | 5,000,000 | 5,000,000 | 0 | Truly populated scan/render workload |
| `sparse-1m-x50` | 50,000,000 | 60,006 | 0 | Large coordinate space with explicit sparsity |
| `formula-100k-x10` | 1,000,000 | 200,000 | 800,000 | Dependency and recalculation workload |

Headers are counted separately in the manifest. Sparse values occur at exact
row and column strides and include the final row and column, so the dimensions
and the low density are both real. Formula inputs and cached values are derived
from row numbers with no random seed or wall-clock data.

The `ci` profile is moderate enough to traverse the installed agent-analysis
path. Its dimensions count the header row because the worker's 250,000-cell
limit applies to the complete used range:

| Fixture | Used-range cells | Values | Formulas | Shape |
|---|---:|---:|---:|---|
| `dense-ci` | 240,020 | 240,000 | 0 | Fully populated scan |
| `sparse-ci` | 240,020 | 605 | 0 | Same coordinate space, less than 1% populated |
| `formula-ci` | 20,010 | 4,000 | 16,000 | Under both cell and 20,000-formula limits |

## Hosted agent-analysis evidence

The compiler-free production-install job generates all three `ci` FODS
sources, converts them to XLSX with the job's installed LibreOffice, and runs
`analyze_workbook` on the dense, sparse, and formula workbooks through the
installed `omasheets` launcher. The harness measures the launcher, Bubblewrap
boundary, Python/UNO worker, and LibreOffice descendants as one foreground
tree. It fails the job unless each command exits successfully without timing
out and Linux supplies non-null positive peak RSS, PSS, and USS plus at least
two observed processes.

The `omasheets-agent-performance-linux-x86_64` artifact contains the bounded
measurement JSON and a fixture manifest. The manifest records deterministic
source hashes and the exact observed hashes, sizes, format-conversion tool, and
filenames for the generated XLSX files. LibreOffice conversion output is
treated as an observed artifact, not claimed to be byte-reproducible. This job
provides a comparable historical evidence stream; it does not by itself turn
one hosted-run result into a production performance guarantee.

## Baseline matrix

Capture cold and warm runs for each engine candidate and each fixture. At a
minimum record first useful result, total wall time, peak PSS, peak USS, peak
RSS, process count, output hash, and whether recalculation and save/reopen were
performed. Compare against the same immutable fixture hashes.

Do not collapse UI residency, query latency, calculation, import, export, and
save/reopen into one number. Those stages answer different product questions.
The first useful baseline should cover:

1. native window cold open, first paint, idle, and scroll;
2. workbook-wide audit;
3. a grouped aggregation or materialised pivot;
4. formula recalculation;
5. staged save and reopen verification; and
6. cancellation latency under a large request.

The performance targets are gates to evaluate after evidence exists, not
current product claims.
