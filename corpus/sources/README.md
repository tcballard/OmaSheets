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
