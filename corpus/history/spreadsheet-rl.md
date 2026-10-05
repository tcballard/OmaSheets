# Historical Spreadsheet-RL sample

This is an archived measurement of commit `371fc30`, not a score of this branch.
The original local manifest and pinned archive register were not committed and
are unavailable here. These figures cannot currently be reproduced and are not
part of the registered corpus or evidence for this change. Preserve the original
manifest digest in the JSON record; do not substitute a different sample.

Source: [Spreadsheet-RL](https://huggingface.co/datasets/Spreadsheet-RL/Spreadsheet-RL) on Hugging Face, released with the Spreadsheet-RL paper and code drop dated 2026-05-17. License: CC-BY-SA-4.0.

The measured set is a 16-file sample of workbooks from that release (Excel-forum and SpreadsheetBench tasks). Workbooks are not vendored. This sample is not the Enron corpus and not the full 33,015-file zip. The full zip was not scored.

## Results

[`spreadsheet-rl.score-summary.json`](spreadsheet-rl.score-summary.json) is the aggregate two-lane score of that 16-file sample. Engine commit `371fc30` (`371fc304e108a3eb2808a121f01ec471f05458b8`), scored 2026-09-24. The record is aggregate only: no workbook paths, cell contents, or per-file results.

The owned lane opened every workbook. The Formualizer candidate lane opened 11 of 16, so its formula-cell totals cover only those 11. The five candidate opens that failed stay in the file count (3 undefined-name or formula-parse, 2 other). None timed out.

| Owned engine lane | Spreadsheet-RL sample (`371fc30`) |
|---|---:|
| Workbooks opened | 16 / 16 |
| Formula cells observed | 14,391 |
| Loaded and compared | 13,703 (95.22%) |
| Stored values matched | 11,151 |
| Match rate of compared | 81.38% |
| Stored values mismatched | 2,552 |
| Not compiled | 688 |

### Refusals

Of the 688 formula cells the owned engine did not compile, the first-failure groups are:

| Reason | Formula cells |
|---|---:|
| Structured reference | 628 |
| Cycle | 53 |
| Unsupported function | 4 |
| Invalid reference | 3 |

| Unsupported function | Formula cells | Workbooks |
|---|---:|---:|
| `GETPIVOTDATA` | 4 | 1 |

The three invalid references are column references without a row. No sheet was skipped, and the owned lane rejected no workbook for the 1904 date system or the two-million-cell limit.

Stored-value mismatches are a separate count from refusals. Of the 2,552 compared cells that disagreed, the largest kind gaps are blank against a number (1,690) and a number against `#VALUE!` (628). Kind labels are not cell contents.

The candidate lane observed 5,264 formula cells in the 11 workbooks it opened and parsed all of them. It also observed 18,204 value cells in those workbooks. That formula total is not comparable with the owned lane's 14,391, because five workbooks never opened on the candidate lane.

These figures describe one 16-file sample scored once. They are not the frozen Enron 1,000-workbook score and not a measurement of the 33,015-file zip.

The score ran on Darwin arm64. The probe address-space limit cannot be applied on this host (`RLIMIT_AS` returns `EINVAL`), so that call was allowed to succeed and no engine source was changed. Wall time for the score command was 1.67479 seconds. Lane resident-set peaks reported by the scorer were 66,453,504 bytes (candidate) and 30,752,768 bytes (owned).
