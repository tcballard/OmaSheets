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
4. Run `omasheets window sample.xlsx` and verify the title bar identifies
   OmaSheets rather than Calc.
5. Select a cell and a dragged range; type a value and a formula; copy and
   paste; undo and redo; toggle bold; and switch every sheet.
6. Scroll horizontally and vertically with mouse, touchpad and keyboard. Check
   that tiles repaint without stale seams and the address/formula surfaces track
   the selected cell.
7. Save a copy, reopen it in both OmaSheets and Calc, and confirm values,
   formulas, formats and sheet inventory survive. Confirm an existing output is
   never replaced without a separate explicit workflow.
8. Open the PPM and compare the first visible region against Calc. Record load
   and render latency separately for cold and warm runs.
9. Repeat with `.xlsx`, `.xlsm`, and `.ods`, a non-ASCII filename, and a broken
   workbook. Confirm failures never alter the source or an existing output.
10. Open a workbook with at least 100,000 used rows and 50 used columns. Confirm
    initial load stays below 15 seconds, first paint below 5 seconds, and RSS
    below 1 GiB; record the actual hardware and measured values.
11. Drag both scrollbars continuously for 10 seconds. Confirm controls remain
    responsive and no stale tile seams remain after input stops.

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
2. Read `omasheets://window` while changing cells and sheets in the native
   window. Confirm address, formula, sheet, zoom and visible rectangle follow
   within 100 ms, and confirm the resource contains no filesystem path.
3. Make an unsaved value and formula edit, then use `read_range`. Confirm the
   result reports `document_source: live_window` and contains both edits while
   the source file hash remains unchanged.
4. Describe the workbook, read a range, search, and trace one formula.
5. Stage scalar value, formula, formatting and bounded range changes. Confirm
   the native diff overlay opens, shows red/green before-and-after records and
   flags any destructive operation. Confirm hiding it does not change the
   workbook and that a proposal over 200 visible changes states it is truncated.
6. Change a different live cell after staging and invoke the MCP apply handoff;
   confirm semantic fingerprint drift rejects the plan. Restage without further
   edits and confirm the handoff returns local review instructions
   and still does not write a workbook.
7. From the overlay choose **Approve & Save a Copy**. Cancel the confirmation
   first and confirm no output appears. Repeat, approve a new destination, and
   confirm the verified copy appears, the overlay closes and the original and
   open in-memory workbook remain unchanged. Also repeat the terminal review,
   enter a wrong token and confirm that path still writes nothing.

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
