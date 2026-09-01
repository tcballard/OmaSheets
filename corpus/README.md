# OmaSheets calculation corpus

The M0 corpus harness measures candidate native calculation engines before any
candidate is promoted into the installed product. Real workbooks are fetched
locally and are never vendored in this repository.

`omasheets-corpus` separates each workbook into a bounded child process. It
checks the file against an immutable SHA-256 manifest before opening it, applies
a per-file timeout and a 2 GiB address-space ceiling on Unix, caps child output,
omits paths and cell contents from its report, and continues after an individual
parse or evaluation failure.

## Create an immutable local manifest

Place `.xlsx` inputs below one root without symbolic links, then run:

```bash
cargo run --locked --release -p omasheets-corpus -- \
  index /path/to/corpus /path/to/manifests/enron-sample.jsonl
```

The manifest contains only a bounded identifier, relative path, and SHA-256 for
each workbook. Review and freeze that file before using it for comparisons.
Indexing is a convenience; a curated manifest can use the same JSONL schema:

```json
{"id":"enron-0001","path":"nested/workbook.xlsx","sha256":"<64 lowercase hex characters>"}
```

## Score a candidate

```bash
cargo run --locked --release -p omasheets-corpus -- \
  score /path/to/manifests/enron-sample.jsonl /path/to/corpus \
  /path/to/reports/enron-formualizer.json \
  --timeout-seconds 30 --require-all
```

`--require-all` returns a non-zero status when any workbook fails or times out,
after writing the complete bounded report. Omit it when collecting an initial
failure distribution.

The first candidate uses Formualizer's Calamine adapter and reports workbook
open/load/evaluation timing plus observed and loaded formula counts. Stored
value parity is explicitly reported as `not_implemented`; an open/evaluate
success must not be presented as the roadmap's recalculation-parity gate.

The separate `omasheets-xlsx-score` lane now loads supported formulas into the
owned engine and compares its calculated results with cached source values. CI
exercises that path on the deterministic 250-formula fixture. This establishes
the comparison contract; it is not the real-corpus parity gate until frozen
Enron/EUSES manifests are scored through the same lane.

## Corpus policy

- Record the upstream corpus name, retrieval date, license or access terms, and
  sampling method beside every frozen manifest.
- Do not commit source workbooks, extracted cell contents, local paths, or model
  prompts.
- Keep Enron and EUSES scores separate as well as combined.
- A changed workbook requires a new manifest entry; never update a digest in
  place and call it the same benchmark.
- Use `scripts/performance.py run` around the release binary when collecting
  process-tree memory and wall-time evidence.
