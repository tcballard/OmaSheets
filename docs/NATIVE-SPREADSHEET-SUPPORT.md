# Native spreadsheet support

The native grid now has a formula bar, address navigation, find/replace,
selection statistics, undo/redo, relative formula fill, sheet management,
row/column editing, sorting, filters, duplicate removal, notes, conditional
highlights and bar/line/pie charts. Presentation is saved in native events and
replayed with stable row and column identities.

Press **Ctrl+Space** (or **Ctrl+Shift+P**) to open the command menu. It replaces
the toolbar and menu bar, with Workbook, Edit, Format, Data, Sheet, View, Agent
and Help categories. Type to search all commands from the root, or search
within a category. A single Commands button also opens it with a pointer.

The interaction follows [Omarchy's menu](https://github.com/omacom/omarchy/blob/148945000fa5bea240864fde9ab551df49f16da6/shell/plugins/menu/Menu.qml):
arrows or Tab/Shift+Tab navigate, Enter chooses, and Right enters a category.
Left or Backspace returns to the parent when the search is empty. Escape
clears the search, then closes the menu; the opening shortcut also closes it.
Unavailable commands are dimmed in categories and omitted from search.
Super+Space remains the desktop's Omarchy menu shortcut.

Direct shortcuts include Ctrl+B/I/U for bold/italic/underline, Ctrl+1 for
formatting, Ctrl+D/R for fill down/right, Ctrl+F for find/replace and Ctrl+G
for a cell or range. Existing workbook, clipboard, undo and agent shortcuts
remain available. The menu displays shortcuts and F1 opens the complete guide.
Dialogs use Tab/Shift+Tab, Space, Enter and Escape, including find results and
exact colour entry. Tab also focuses proposal details, with a visible outline;
arrows, Page Up/Down and Home/End scroll through a long review before a decision.
Opening and cancelling the menu preserves the active
editor and draft; running a command retains the existing failed-save and
confirmation checks. No shell commands are taken from search text.

## Interchange

Native XLSX import/export preserves explicit RGB foreground/fill colours,
bold/italic/underline, supported font sizes, horizontal alignment, wrapping,
all/bottom borders, saved number-format strings, custom row/column dimensions,
rectangular merges, gridline visibility and frozen panes. Styled blank cells
are retained. Row height converts between points and 96-dpi pixels; column
width assumes a seven-pixel maximum digit width, per SpreadsheetML's documented
conversion. Native and exported values remain numeric when formatted.

Native display supports General, up to three decimal places, grouping, simple
£/$/€ currency and percent patterns, and the listed date formats. Other saved
number-format strings use General in the grid and are preserved for XLSX.

Import reports losses for theme/indexed colours, unsupported border layouts,
font effects, row/column default styles, hidden rows/columns, split panes,
source filters, comments and conditional formatting. Font families and default
sheet dimensions use native defaults. Export reports omitted notes, chart
definitions, conditional rules, filters, native checks, watches and history.
LibreOfficeKit remains the compatibility path for complex workbooks, legacy
XLS and read-only XLSM. Conversion never overwrites an existing destination or
changes the source file.

The width calculation follows the [SpreadsheetML column specification](https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.spreadsheet.column?view=openxml-3.0.1).

## Formula and editing boundaries

The parser registry contains 104 function names, including TEXTJOIN, PV, IRR,
covariance and standard-normal distribution functions. Bounded array constants
work in aggregates and lookups without spilling into neighbouring cells. The
registry and [function list](FUNCTIONS.md) are checked together. Clock and random
functions remain explicitly refused: calculation does not read hidden time or
randomness. The core's stored tick events have not been connected to volatile
formula evaluation.

Formula history retains the original text. Editable views and XLSX export
project stable references to current A1 addresses, preserving absolute-axis
markers and renamed sheet references. A stable range that no longer has a
faithful rectangular spelling blocks copy/fill/duplication and exports its
calculated value with a disclosure. Undo restores compiled bindings directly.

Ordinary cell, formatting, sort, filter and chart edits participate in bounded
undo. Structural changes start a new undo history; destructive commands say so
before applying. Duplication is bounded to 900 occupied cells and refuses native
tables. Bulk cell edits/fill/copy are bounded to 1,000 cells. Presentation is
bounded to 10,000 styled cells, 10,000 row heights, 1,000 column widths and merges,
32 conditional rules, and 16 charts per sheet. Larger or unsupported operations
are refused without partial commits. Sorting moves entire rows and refuses
merged ranges. Native filter matching is text containment; replacement changes
text literals, preserving formulas.

## Verification

The service tests exercise atomic/stale edits, durable styles, sort and formula
identity, undo after structural movement, merge/freeze anchors, Unicode
replacement, chart values, independent sheet duplication, and exact formula
projection. Agent tests cover atomic proposals, combined-state checks, review
revision guards, approval, rejection, reopen and review truncation.

`scripts/check_native_interchange.py` runs the real installed service and uses
OpenPyXL 3.1.5 as an independent XLSX writer/reader. It checks styled blanks,
formulas and caches, dimensions, merges, freeze, chart data, reopen and refusal
to overwrite. The ignored `independent_reader` Rust integration test runs the
same public service API without a socket for environments that prohibit
listeners; it requires Python with OpenPyXL.

CI captures the real Qt viewport and proposal review and verifies installed
bundles and Arch package workflows. Container and CI results are wiring and
regression evidence; no Dell or Omarchy hardware acceptance is claimed. No
larger corpus has been rescored here, so this change claims no aggregate corpus
delta. The existing observed/loaded/compared denominator remains explicit.

`spikes/qt-grid/tests/qml` exercises the production QML with Qt Quick Test and
an in-memory service-boundary fixture. It sends real key events through search,
category navigation, shortcuts, modal dialogs, cancellation and failed writes.
Run it with `qmltestrunner -input spikes/qt-grid/tests/qml -import
spikes/qt-grid/tests/qml/mocks` in a graphical session (or under `xvfb-run`).
The installed application is also captured with the command menu and search
results open. These checks complement the real service workflows above.

### Recorded calculation fixtures

The [8 September container measurements](evidence/native-completion-2026-09-08/calculation.json)
use 100,000 formulas and 20 edits per fixture, with the source commit and
executable digest recorded. Full-chain p95 recalculation was 38.9 ms, fan-out
32.1 ms, and a 1,000-cell sparse dirty closure 0.24 ms. Peak child RSS was
43–46 MiB. These measure the calculation engine, not end-to-end grid latency.
