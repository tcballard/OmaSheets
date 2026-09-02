# Corpus source registers

Each `*.json` file here is a source register for `scripts/fetch_corpus.py`
(schema in `../README.md`). A register pins one archive by URL and SHA-256 and
records its license or access terms, retrieval date and sampling method. The
frozen JSONL manifest built from that archive lives beside it under the name
the register's `manifest` field gives.

Fetched archives, extracted workbooks and cell contents stay on the local
machine. A register may be added only after the project owner has confirmed
the source's license or access terms.

## Registered

- `enron-figshare.json` / `enron-figshare.jsonl`: Felienne Hermans, "Enron
  Spreadsheets and Emails", figshare article 1221767 (DOI
  10.6084/m9.figshare.1221767.v2), `spreadsheets.7z`, CC BY 4.0 as declared
  by figshare. Owner confirmed the source on 2026-09-01. The archive holds
  15,871 `.xlsx` and 58 `.xls` workbooks extracted from the public FERC Enron
  release; the manifest freezes 1,000 unique `.xlsx` workbooks chosen by
  `scripts/sample_corpus.py`. Member names carry the mailbox owner's name, as
  in the upstream dataset.

## Results

`enron-figshare.score-summary.json` records the aggregate two-lane score of
the frozen sample (engine commit, wall time, process-tree peak memory, both
lane summaries, the owned lane's unsupported-function distribution and the
failure kinds). Scored on 2026-09-01 on a 4-vCPU Linux container in 243 s:

| Lane | Opened | Formula cells | Loaded | Compared | Matched |
|---|---|---|---|---|---|
| Formualizer candidate | 834 / 1000 | 419,854 | 419,854 (100%) | not implemented | not implemented |
| Owned M0 engine | 971 / 1000 | 730,879 | 600,171 (82.1%) | 600,171 | 542,159 (90.3%) |

The owned lane's 130,708 uncompiled cells split into syntax 52,523,
unsupported function 44,259, invalid reference 24,123, unknown sheet 9,747
and cycle 56. The unsupported functions by formula cells are led by CHOOSE
(28,995 cells, 6 workbooks) and SUBTOTAL (9,965 cells, 24 workbooks); by
workbooks they are led by NOW (97), TODAY (28), SUBTOTAL (24) and CELL (15).
Mismatches are concentrated in 108 workbooks. These are real-corpus numbers
for one frozen sample, not a product claim; the formula-gap runbook ranks its
next tranche from this file.

## Not registered

- EUSES: distributed under request-based access terms without a clear
  redistribution license. It stays out until the owner supplies a source and
  terms they accept.

Reproduce a frozen sample on a clean machine with:

```bash
python3 scripts/fetch_corpus.py corpus/sources/enron-figshare.json /path/to/corpus
python3 scripts/sample_corpus.py /path/to/corpus/enron-figshare /tmp/enron.jsonl \
  --count 1000 --prefix enron
cmp /tmp/enron.jsonl corpus/sources/enron-figshare.jsonl
cargo run --locked --release -p omasheets-corpus -- \
  verify corpus/sources/enron-figshare.jsonl /path/to/corpus/enron-figshare
```
