# v0.0.1 acceptance runbook

Run this on an Omarchy `quattro` workstation. Automated Arch CI covers policy,
transactions, the pinned plugin validator, wheel packaging, user-local install,
native launch under Xvfb, live bridge, diff overlay, provenance and uninstall.
The native/agent job also exercises evidence-cited planning, structural rows,
formula fill, sorting, plan supersession and the non-publishing apply handoff.
It does not prove a real Quickshell/Wayland session, physical input,
accessibility, desktop portal behavior or file-manager integration.

## Preconditions

```bash
omarchy plugin add https://github.com/tcballard/OmaSheets.git --enable
~/.config/omarchy/plugins/io.github.tcballard.omasheets/bin/omasheets-plugin install
omasheets --version
omasheets doctor
omarchy plugin validate ~/.config/omarchy/plugins/io.github.tcballard.omasheets
```

`doctor` must report Bubblewrap, LibreOffice, Python UNO, the native engine,
desktop integration and the Omarchy plugin as ready. Confirm Codex lists the
OmaSheets personal plugin after a refresh/restart and can start its MCP server.

Use disposable workbooks containing formulas, formatting, a chart, multiple
sheets, and one filename with spaces and shell punctuation. Record SHA-256
hashes before every mutation test.

## Native LibreOfficeKit window on Wayland

1. Run `omasheets lok status` and confirm all three checks are ready.
2. Run `omasheets lok render sample.xls --output /tmp/sample-tile.ppm`.
3. Run `omasheets window sample.xlsx` and verify the title bar identifies
   OmaSheets rather than Calc.
4. Select a cell and a dragged range; type a value and a formula; copy and
   paste; undo and redo; toggle bold; and switch every sheet.
5. Scroll horizontally and vertically with mouse, touchpad and keyboard. Check
   that tiles repaint without stale seams and the address/formula surfaces track
   the selected cell.
6. Save a copy, reopen it in both OmaSheets and Calc, and confirm values,
   formulas, formats and sheet inventory survive. Confirm an existing output is
   never replaced without a separate explicit workflow.
7. Open the PPM and compare the first visible region against Calc. Record load
   and render latency separately for cold and warm runs.
8. Repeat with `.xlsx`, `.xlsm`, and `.ods`, a non-ASCII filename, and a broken
   workbook. Confirm failures never alter the source or an existing output.
9. Open a workbook with at least 100,000 used rows and 50 used columns. Confirm
    initial load stays below 15 seconds, first paint below 5 seconds, and RSS
    below 1 GiB; record the actual hardware and measured values.
10. Drag both scrollbars continuously for 10 seconds. Confirm controls remain
    responsive and no stale tile seams remain after input stops.

Passing this section supplies the hands-on Wayland evidence CI lacks. It does
not prove Excel equivalence or close the remaining accessibility, dialog and
crash-containment limitations.

## Native opening and selection

1. Double-click `.xls`, `.xlsx`, `.xlsm`, and `.ods` files in the file manager.
2. Confirm `.xlsx` and `.ods` open editable in the OmaSheets window, while
   `.xls` and `.xlsm` open read-only and preserve their source bytes.
3. Run `omasheets select sample.xlsx` and open the bar panel.
4. Confirm the panel shows only the basename and format, never a filesystem
   path, and that right-click opens the exact selected workbook.

## Agent read and staged change

1. Choose **Ask Codex** in the native header and Omarchy panel. Confirm each
   opens Codex with no workbook path in its initial prompt.
2. Read `omasheets://agent` and confirm it contains the selected workbook's
   public session and current focus but no source path or publication authority.
3. Start `omasheets mcp serve` through another MCP client if needed.
4. Read `omasheets://window` while changing cells and sheets in the native
   window. Confirm address, formula, sheet, zoom and visible rectangle follow
   within 100 ms, and confirm the resource contains no filesystem path.
5. Make an unsaved value and formula edit, then use `read_range`. Confirm the
   result reports `document_source: live_window` and contains both edits while
   the source file hash remains unchanged. Retain each returned `evidence_id`.
6. Describe the workbook, read a range, search, and trace one formula. Confirm
   a plan cannot cite evidence from another session or semantic live snapshot.
7. Stage scalar value, formula, formatting, bounded range, inserted row,
   reference-aware fill and bounded sort changes. Confirm the native diff
   overlay opens and shows the goal, explanation, assumptions, purpose groups,
   red/green before-and-after records, and destructive flags. Confirm hiding it
   does not change the workbook and that a proposal over 200 visible changes
   states it is truncated.
8. Give a correction such as “exclude Forecast” and confirm `revise_plan`
   creates a new plan, the prior plan becomes `superseded`, and only the new
   plan is actionable.
9. Change a different live cell after staging and invoke the MCP apply handoff;
   confirm semantic fingerprint drift rejects the plan. Restage without further
   edits and confirm the handoff returns local review instructions
   and still does not write a workbook.
10. From the overlay choose **Approve & Save a Copy**. Cancel the confirmation
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

The maintainer has reported completing a hands-on test. Record the tested
commit, Omarchy version, hardware, files, measurements and pass/fail notes here
before treating every numbered item above as release evidence.

## Removal

1. Add an unrelated MIME association and a separate personal Codex marketplace
   entry after installation.
2. Run `~/.local/bin/omasheets uninstall`.
3. Confirm OmaSheets application bytes, launcher, Codex plugin and associations
   are removed; the unrelated edits remain; and an independently modified
   desktop entry is preserved and reported as a conflict rather than deleted.
4. Resolve any reported conflict, repeat uninstall, then run
   `omarchy plugin remove io.github.tcballard.omasheets`.
