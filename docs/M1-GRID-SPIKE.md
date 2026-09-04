# M1 native grid spike

**Status:** implementation candidate; dependency acceptance and on-device review
are not complete.

## Decision under test

Use a standalone Qt 6 / Qt Quick window with a Rust model connected by CXX-Qt.
This fits Omarchy's Qt/QML shell environment while keeping OmaSheets a normal
application window rather than adding long-running spreadsheet state to the
desktop shell. The current LibreOfficeKit window remains the compatibility path.

The spike is isolated from the release workspace. Promotion requires an explicit
review of customization, accessibility, interaction latency, scrolling, memory,
packaging, and the ongoing Qt dependency cost.

## Scope

- Synthetic table: 1,000,000 rows by 64 columns.
- Viewport-only QML delegates backed by a Rust model.
- Keyboard movement, page and edge jumps, mouse selection, and in-cell editing.
- Accessible grid and visible-cell names, descriptions, focus, and selection.
- Live inheritance of Omarchy's active semantic palette, with a standalone
  fallback, using standard Qt Quick controls.
- Multi-sheet native-document loading through the authenticated local service,
  with themed tabs, Ctrl+Page Up/Down switching and a bounded eight-tile cache
  over sparse 64 × 16 grid pages.
- Native value/formula editing appended through the service; calculated display
  values remain separate from editable formula source.
- Repeatable headless wiring smoke plus a manual on-device evidence run.

The spike now connects to `.omasheets` documents without making a Unix-socket
round trip for every painted cell. Reads and edits use the selected sheet's
stable ID, and switching clears the page cache before repainting. Row/column
insertion and production error recovery remain outside the candidate
integration.

## Non-gating CI observation

CI run 167 exercised the synthetic 1,000,000-row by 64-column fixture in a cold
process on a GitHub-hosted Ubuntu 24.04 VM (4 vCPU, Intel Xeon Platinum 8573C),
Qt 6.4.2, Xvfb and forced Mesa software OpenGL. After 30 warm-up frames, the
exact 180-frame interval metrics were 24.119257 ms p95 and 24.967170 ms worst;
286 delegates were live at report time and the Rust model served 106,149 cell
reads. Process peak RSS was 211,672 KiB. These figures prove the headless harness
is wired and the live delegate count is bounded. They are not the declared
eight-core Omarchy baseline, not a hardware-composited run, and not an M1 target
result.

## Review method

Build and run the commands in `spikes/qt-grid/README.md` on the declared
eight-core Omarchy baseline. Record CPU, RAM, GPU, display resolution and scale,
Qt version, Omarchy revision, renderer, and whether each run is cold or warm.

Check all of the following with the keyboard and then a screen reader:

1. Move with arrows, Page Up/Down, Home/End, and Ctrl+Home/End; switch tabs with
   Ctrl+Page Up/Down.
2. Start editing with Enter and F2; commit with Enter and cancel with Escape.
3. Select and edit with the mouse without losing keyboard focus.
4. Open a multi-sheet native document, switch by keyboard and mouse, edit a
   literal and formula on the second sheet, restart the window and verify both
   edits persisted on that sheet and the formula bar retained formula source.
5. Verify the grid name, current cell address/value/type, selection, and edit
   state are announced.
6. Switch between the maintainer's normal dark, light and one customized
   Omarchy theme while the window remains open; verify the palette updates.
7. Verify headers, selection, formulas, status colours, editing, scrollbars and
   scaling remain legible at the maintainer's normal display scale.

## Evidence record

| Field | Result |
| --- | --- |
| Baseline hardware / display | _not recorded_ |
| Omarchy / Qt / renderer | _not recorded_ |
| Customization review | _not run_ |
| Active-theme review | _not run_ |
| Native document read/edit persistence | _not run_ |
| Multi-sheet switching and cache isolation | _not run_ |
| Keyboard-only review | _not run_ |
| Screen-reader review | _not run_ |
| Warm scroll, synthetic 1,000,000 × 64 | _not measured_ |
| Keystroke to committed paint | _not measured_ |
| Idle RSS, native 100,000-cell document | _not measured; document adapter absent_ |

## Decision

- [ ] Accept Qt 6 / Qt Quick / CXX-Qt for the M1 native grid.
- [ ] Reject it and record the failed gate and next candidate.

Decision owner, date, and notes: _pending maintainer review_.
