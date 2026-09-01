# Corpus source registers

Each `*.json` file here is a source register for `scripts/fetch_corpus.py`
(schema in `../README.md`). A register pins one archive by URL and SHA-256 and
records its license or access terms, retrieval date and sampling method. The
frozen JSONL manifest built from that archive lives beside it.

No source is registered yet. Adding one requires the project owner to confirm
the license or access terms first: the Enron spreadsheet corpus and the EUSES
corpus are distributed under different terms, and neither may be vendored,
mirrored or summarised cell-by-cell in this repository. Fetched workbooks and
extracted contents stay on the local machine.
