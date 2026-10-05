# Linked-workbook correctness comparison — 5 October 2026

Compared main `15896e5744ce` with candidate `f1578995b885` using the unchanged,
hash-verified 1,000-file Enron manifest. Both builds used the release owned-only
corpus scorer, a 30-second per-file timeout and sequential runs.

| Metric | Main | Candidate |
| --- | ---: | ---: |
| Workbooks opened | 998 / 1,000 | 998 / 1,000 |
| Failed / timed out | 2 / 0 | 2 / 0 |
| Formula denominator | 980,734 | 980,734 |
| Formulas loaded and compared | 881,666 | 919,896 |
| Formula coverage | 89.8986% | 93.7967% |
| Cached values matched | 877,770 | 915,900 |
| Cached values mismatched | 3,896 | 3,996 |
| Match rate among compared | 99.5581% | 99.5656% |

This adds 38,230 evaluated formulas and 38,130 matched cached values. It remains
below the unchanged 97% formula-coverage gate. The older 89.75% report used
924,235 observed formulas and 996 opened files; improved import handling on main
now opens two more files. Compare the same current denominator above, not rounded
rates across different accepted-workbook sets.

Review found and fixed native import silently inventing missing linked values,
Unicode/short file-URL panics, and unavailable cached inputs contaminating
previously matching formulas. Native document import now preserves external
formula source caches through reopen; owned scoring only evaluates references
with available external inputs.

One workbook still loses 34 previously matching results: its 76 differences are
cached `#VALUE!` becoming numbers after linked SUMIF calculations and dependent
arithmetic. Microsoft documents closed-workbook SUMIF/SUMIFS returning `#VALUE!`:
https://support.microsoft.com/en-us/excel/how-to-correct-a-value-error-in-the-sumif-sumifs-function
That is consistent with the observed shape, but the original calculation state
is unknown, so these remain counted as mismatches, not waived or declared fixed.
The other 24 net new differences are 14 numeric and 10 text-to-error results.

Process-tree RSS was unavailable in this execution environment. Per-probe RSS
in the aggregate JSON is not a replacement for that metric. This is correctness
evidence only; the resource gate and real-device acceptance remain outstanding.
The legacy scorer was deliberately disabled and its empty lane is not a failure
of the owned engine. No workbook contents or per-file source paths are published.

See [aggregate evidence](2026-10-05-linked-workbooks.json).
