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
the frozen sample for the current engine (engine commit, wall time,
process-tree peak memory, both lane summaries, the owned lane's
unsupported-function distribution and the failure kinds), and
`enron-figshare.score-delta.json` records the owned lane against the
baseline engine that first scored the sample. Both are aggregate only. Scored
on 2026-09-01 on a 4-vCPU Linux container:

Generate both files from a measured schema-2 scorer report with
`scripts/update_corpus_summary.py`. The command validates the manifest digest,
successful performance wrapper result and baseline identity, then emits only
aggregate failure classes and metrics. Use `--help` for the complete evidence
metadata and resolved-class options; the baseline summary may be the checked-in
summary copied aside before a new score.

```bash
cp corpus/sources/enron-figshare.score-summary.json /tmp/enron-before.json
python scripts/update_corpus_summary.py \
  --score OUT/score.json --performance OUT/performance.json \
  --manifest corpus/sources/enron-figshare.jsonl \
  --baseline-summary /tmp/enron-before.json \
  --summary corpus/sources/enron-figshare.score-summary.json \
  --delta corpus/sources/enron-figshare.score-delta.json \
  --runner "linux x86_64, eight-core baseline, cold run"
```

| Owned M0 engine lane | Baseline (`08f38d1`) | After formula gaps (`0eccaff`) |
|---|---|---|
| Workbooks opened | 971 / 1000 | 971 / 1000 |
| Formula cells observed | 730,879 | 730,879 |
| Loaded (compiled) | 600,171 (82.1%) | 662,234 (90.6%) |
| Matched of compared | 542,159 (90.3%) | 648,639 (97.9%) |
| Mismatched | 58,012 | 13,595 |
| Not compiled | 130,708 | 68,645 |
| Wall time, both lanes | 243 s | 270 s |
| Process-tree peak RSS | 1.5 GiB | 2.1 GiB |

The Formualizer candidate lane is unchanged: 834 opened, 419,854 formula
cells parsed. Of the 68,645 cells the owned engine still does not compile,
39,987 reference other workbooks (never evaluated), 16,085 call unsupported
functions (led by `_xll` add-in calls, `CALCSKEW`, `RAND`, `OFFSET`), 5,765
use defined names whose definitions do not compile, 4,574 are invalid
references, 1,836 are cycles and 278 are other syntax. The remaining
mismatches are concentrated: 9,125 of 13,595 sit in one workbook. These are
numbers for one frozen sample of one corpus, not a product claim.

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
