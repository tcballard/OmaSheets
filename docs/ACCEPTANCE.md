# Owned-engine acceptance

Automated CI builds and installs OmaSheets on Arch without LibreOffice. It
checks ELF/shared-library dependencies, default XLSX opening, isolated Rust
workbook jobs, owned PDF previews, export/reopen, reviewed agent changes and
installation/removal. REA reference rendering runs separately and does not
establish product runtime compatibility.

## On Omarchy

1. Install a package or bundle built from the exact reviewed commit, run
   `omasheets doctor`, and retain its provenance and dependency report.
2. Confirm `libreoffice` and `soffice` are absent, then run `omasheets` and use
   **Try an example**. Edit a value and formula, close, reopen and compare the
   persisted calculated values and native digest.
3. Run `omasheets launch sample.xlsx` for a supported workbook. Verify it opens
   in the Qt grid, the original SHA-256 remains unchanged, and the durable
   `.omasheets` working copy survives closing the window.
4. Repeat through the file manager and the app's Excel/import commands. Try an
   unsupported formula, named range, macro, external link and pivot source;
   each must refuse before a native copy is published. XLS/XLSM/ODS must refuse.
5. Export a new XLSX copy. Independently inspect formula text, calculated values
   and supported styles. A formula whose stable binding cannot be represented
   must refuse before writing an output. Review the export's native-only
   metadata limitations; an XLSX projection does not preserve native history.
6. Open two windows sharing the owned service. Close the window that started it
   and verify the other remains usable. Close both and verify a transient
   service exits. An independently started user service must remain running.

## Agent jobs and native review

1. Use **Ask Agent** with the configured Omarchy default agent. Read
   `omasheets://session` or `omasheets agent-session resource`. Check that it
   contains live native selection/overview without a filesystem path.
2. Use `native_read` and `native_lineage` after a human cell edit. Propose a change
   with `native_propose`. Verify only its branch changes; inspect derived values,
   lineage and checks through `native_review` and the Qt review panel.
3. Cancel, approve and reject separate proposals. A failed check or stale
   revision must block approval. Confirm approval and rejection survive reopen.
4. Separately select an immutable supported XLSX using `omasheets select`.
   Describe, read, search, trace, query and audit it through selected-file tools.
   A query batch must share one sealed evidence record and preserve order; an
   invalid subquery must create no partial observation.
5. Stage supported value/formula/range/format or structural changes. Inspect
   sealed source identity, before/after values, export/reopen verification and
   bounded PDF preview. Selected-file fill/chart/pivot operations must refuse.
6. Approve a new copy locally, verify source preservation, and independently
   read exported cells/formulas. Test explicit replacement/undo on disposable
   files; source drift or later edits must block publication/undo.
7. Active native documents use the native service. Selected-file jobs reject
   live SQLite WAL state rather than copying an incomplete database.

## Physical desktop gates

Record the commit, Omarchy/Qt version, machine and fixture identities. Exercise
keyboard editing/navigation, copy/paste, undo/redo, mouse/touchpad scrolling,
sheet changes, themes and 100/150/200% display scale on Wayland. Inspect actual
accessibility roles, labels and focus. Measure cold open, editing latency,
continuous scroll and idle memory on the declared hardware. Xvfb screenshots
do not establish these physical desktop or accessibility results.

## Removal

Add unrelated MIME and Codex plugin entries, uninstall OmaSheets, and confirm
owned launchers/application/integration are removed while workbooks and those
unrelated entries remain. A user-modified owned file must be preserved and
reported as a conflict. Upgrading an older installation must remove retired
LOK executables without deleting workbooks or modifying their source bytes.
