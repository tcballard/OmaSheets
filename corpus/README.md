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

Each workbook is scored through two isolated lanes, each in its own bounded
child process:

- the Formualizer/Calamine candidate reports workbook open/load/evaluation
  timing plus observed and loaded formula counts;
- the owned M0 engine (`omasheets-xlsx`) loads every formula it can compile,
  recalculates, and compares each result with the cached source value.

The report (`schema` 2) keeps the lanes apart. `summary` covers the candidate
and `owned_summary` covers the owned engine with, over formula cells, the
parse rate, comparison coverage and stored-value match rate, plus the
unsupported-function distribution by formula cells and by workbooks, the
compile-failure reasons, and the largest peak resident set observed. Read the
four states separately: `opened` (the importer accepted the file), loaded
(the formula compiled), compared (a cached value existed) and matched.
`--require-all` fails when either lane fails or times out on any workbook.

`omasheets-xlsx-score INPUT.xlsx` prints the same owned-lane report for a
single workbook. CI exercises both lanes on the deterministic fixtures; that
establishes the comparison contract, and it is not real-corpus evidence until
frozen Enron/EUSES manifests are scored through the same lanes.

## Verify a frozen manifest

```bash
cargo run --locked --release -p omasheets-corpus -- \
  verify /path/to/manifests/enron-sample.jsonl /path/to/corpus
```

`verify` re-hashes every entry, refuses symbolic links and paths outside the
root, prints a per-entry report and exits non-zero on any drift. Run it on a
clean machine before trusting a score.

## Fetch a source reproducibly

`scripts/fetch_corpus.py REGISTER.json DESTINATION` downloads one archive by
`https` or `file` URL, refuses to continue unless its SHA-256 equals the
register's `archive_sha256` (and its MD5 equals `upstream_md5` when the host
publishes one), and extracts only `.xlsx` members: no symbolic links, no
absolute or parent paths, at most 50,000 workbooks of 512 MiB each and 16 GiB
in total, with the extension normalised to lowercase so every file is
indexable. Zip archives use the standard library; `.7z` archives are unpacked
through the `7z` command into a staging directory that is filtered with the
same rules. Existing archives and extraction directories are never replaced.

A source register records what the frozen manifest was built from:

```json
{
  "schema": 1,
  "name": "enron-figshare",
  "url": "https://example.invalid/archive.7z",
  "archive_format": "7z",
  "archive_sha256": "<64 lowercase hex characters>",
  "upstream_md5": "<32 lowercase hex characters, optional>",
  "license": "exact license or access terms, with a link",
  "retrieved": "2026-09-01",
  "sampling": "how workbooks were selected and how many",
  "sample_count": 1000,
  "manifest": "enron-figshare.jsonl"
}
```

## Freeze a sample

A manifest holds at most 1,000 workbooks, so large corpora are sampled:

```bash
python3 scripts/sample_corpus.py /path/to/corpus/enron-figshare \
  corpus/sources/enron-figshare.jsonl --count 1000 --prefix enron
```

`sample_corpus.py` hashes every extracted `.xlsx`, collapses byte-identical
files to the first relative path, sorts the unique digests and keeps the first
`--count`. SHA-256 order is effectively uniform over unique workbooks, so the
sample is reproducible from the same archive without a seed. The manifest
records only the bounded ID, the archive-relative path and the digest.

Registers and frozen manifests for real corpora live in `corpus/sources/`,
each added only after the project owner confirmed the source's license or
access terms. See `corpus/sources/README.md` for what is registered.

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
