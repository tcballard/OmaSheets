# OmaSheets v0.0.1 product contract

## Mission

OmaSheets is the spreadsheet component of the working-name OmaOffice suite. It
should let an Omarchy user retain ordinary office-spreadsheet capability while
giving local agents a safer and more useful interface than screen-driving or
unrestricted file mutation.

## Users

1. A person who needs to open and edit workbooks on an Omarchy Linux host.
2. A local or connected coding agent asked to inspect or change a workbook.
3. A reviewer who needs evidence of exactly what changed and how it was checked.

## Format policy

| Format | Human open | Agent read | Agent stage | Publish |
| --- | --- | --- | --- | --- |
| `.xls` | Yes | Yes | No | Convert to a new `.xlsx` only |
| `.xlsx` | Yes | Yes | Yes | Copy by default; replace only with explicit local approval |
| `.xlsm` | Yes | Yes | No | Never in v0.0.1 |
| `.ods` | Yes | Yes | Yes | Copy by default; replace only with explicit local approval |

Legacy `.xls` and macro-enabled `.xlsm` inputs are never overwritten. Conversion
does not establish Excel equivalence and always returns a manual-review result.

## Human workflow

1. Open a supported workbook from the desktop or file manager.
2. Ask an agent to describe, trace, search, render, or propose changes.
3. Review the sealed plan, source and staged hashes, semantic diff, warnings,
   verification evidence, and rendered preview.
4. Type an exact approval token in the local review surface.
5. Receive a receipt and, for replacement operations, a recoverable backup.

## Non-goals

- Reimplementing a spreadsheet calculation engine.
- Claiming pixel-perfect or formula-perfect Microsoft Excel compatibility.
- Running workbook macros, external-data refreshes, or arbitrary extensions.
- Giving remote MCP clients direct commit or undo authority.
- Editing the workbook currently open in a live Calc process.
- Collaborative editing or cloud document storage.

## Acceptance gates

- A genuine BIFF `.xls` fixture opens and converts without modifying its source.
- Agent requests cannot select arbitrary filesystem paths or invoke commit.
- A staged plan is invalidated if its source, destination, revision, or seal changes.
- Recalculation, reopen, formula-error inspection, and preview rendering run before
  a plan becomes eligible for local approval.
- Copy publication cannot overwrite an existing path.
- Replace publication creates a verified backup and a bounded undo receipt.
- Every publish and undo has a durable, hash-chained receipt.
- The Omarchy surface passes the pinned shell-plugin validator.
- The Python package and installed wheel pass the same protocol and safety suite.
