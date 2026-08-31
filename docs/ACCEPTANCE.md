# v0.0.1 acceptance runbook

Run this on an Omarchy `quattro` workstation. Automated CI covers policy,
transaction, plugin-manifest, MIME rollback, protocol, and wheel contracts.
This runbook covers the real Quickshell, Bubblewrap, LibreOffice UNO, desktop,
and file-manager boundaries that a generic CI container cannot faithfully
exercise.

## Preconditions

```bash
omasheets --version
omasheets doctor
omarchy plugin validate ~/.config/omarchy/plugins/io.github.tcballard.omasheets
```

`doctor` must report the Bubblewrap, LibreOffice, Python, and Python-UNO checks
as ready. Desktop integration and the Omarchy plugin should also be present.

Use disposable workbooks containing formulas, formatting, a chart, multiple
sheets, and one filename with spaces and shell punctuation. Record SHA-256
hashes before every mutation test.

## Experimental LibreOfficeKit rendering

1. Build and install the helper from [`../spikes/libreofficekit/`](../spikes/libreofficekit/README.md).
2. Run `omasheets lok status` and confirm all three checks are ready.
3. Run `omasheets lok render sample.xls --output /tmp/sample-tile.ppm`.
4. Open the PPM and compare the first visible region against Calc. Record load
   and render latency separately for cold and warm runs.
5. Repeat with `.xlsx`, `.xlsm`, and `.ods`, a non-ASCII filename, and a broken
   workbook. Confirm failures never alter the source or an existing output.

Passing this section proves only the rendering spike. It does not promote
LibreOfficeKit to the default human editor.

## Native opening and selection

1. Double-click `.xls`, `.xlsx`, `.xlsm`, and `.ods` files in the file manager.
2. Confirm each opens in LibreOffice Calc through the OmaSheets desktop entry.
3. Run `omasheets select sample.xlsx` and open the bar panel.
4. Confirm the panel shows only the basename and format, never a filesystem
   path, and that right-click opens the exact selected workbook.

## Agent read and staged change

1. Start `omasheets mcp serve` through an MCP client.
2. Describe the workbook, read a range, search, and trace one formula.
3. Stage a scalar value or formula change. Confirm the original hash is
   unchanged and the panel reports the operation count.
4. Invoke the MCP apply handoff. Confirm it returns local review instructions
   and still does not write a workbook.
5. Click **Review in terminal**. Type a wrong token first and confirm no output
   file appears. Repeat and type the exact token; confirm a new `-omasheets`
   copy appears and the original remains unchanged.

## Replacement and undo

1. Stage another change and run the explicit local approval command with
   `--replace`.
2. Confirm a private verified backup and hash-chained receipt exist.
3. Run `omasheets undo <receipt-id>`, type the exact undo token, and confirm the
   original SHA-256 is restored.
4. Modify the published workbook after replacement and confirm undo refuses to
   overwrite those later bytes.

## Legacy conversion

```bash
omasheets convert sample.xls
```

Confirm `sample.xls` retains its original SHA-256, a new adjacent
`sample.xlsx` and PDF preview are created, and the receipt says
`manual_review_required: true` and `excel_equivalence_claimed: false`. Open both
versions and manually compare sheets, formulas, formatting, charts, named
ranges, and any warnings.

## Removal

1. Add an unrelated MIME association after installation.
2. Run `omasheets integrate uninstall`.
3. Confirm OmaSheets associations are removed, the unrelated edit remains, and
   an independently modified desktop entry is preserved with a conflict rather
   than deleted.
