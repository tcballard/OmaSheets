# ADR-0005: Stable identities and content-addressed events in `omasheets-core`

- Status: Proposed (first slice landed behind the M0 engine; not yet the installed product model)
- Date: 2026-09-01

## Context

[`ADR-0003-EVENT-SOURCED-NATIVE-CORE.md`](ADR-0003-EVENT-SOURCED-NATIVE-CORE.md)
commits OmaSheets to an event-sourced native core with stable identities,
attributed edits and deterministic replay. The owned M0 calculation engine
now covers enough of the formula surface to carry a document model, but it
still addresses cells by coordinates, and a coordinate is exactly what an
agent edit must never depend on: inserting a row above a referenced cell must
not silently retarget the formula that references it.

The owner chose, for this slice, to bind formulas to stable cell identities
resolved at event creation, with A1 as input syntax only, rather than to
introduce structured table references first.

## Decision

`crates/omasheets-core` defines the document model and event log:

- **Identities.** Every document, sheet, row, column, table, branch, import,
  proposal, check and watched output has a 128-bit `ObjectId`. New identities
  are derived by SHA-256 from the current head event, the object kind and an
  ordinal, so replaying the same log mints the same identities. Coordinates
  never appear in an identity or a permission.
- **Events.** The envelope carries a schema version, a content-addressed
  `EventId` (SHA-256 over the canonical JSON of the other fields), the parent
  event, the branch, an actor of kind human, import, agent, model-assisted or
  system, a caller-supplied timestamp and one bounded operation. An event is
  self-verifying; a tampered, reordered or foreign-branch event is rejected.
- **Operations** in the first slice: create document; add, rename and delete
  sheets; add and delete rows and columns at a view position; add and rename
  tables; set a column type; set value, set formula and clear cell; record an
  import; an explicit tick; propose, accept and reject proposals.
- **Formulas.** `Document::command` accepts A1 text, parses it with the M0
  engine's parser against the current view, and records the source, the sheet
  names it used and every reference as stable `(sheet, row, column)`
  identities in the engine's traversal order (ranges materialise into bounded
  lists). Replay re-parses the source purely for structure and rebinds each
  reference by position to engine cells addressed by creation ordinals, never
  by view position. Structured table references use the same path: table and
  column names bind to stable `TableId` and `ColumnId` values, while the event
  records the stable cells selected at that point. Adding table rows carries
  the deterministic replacement bindings for existing structured ranges.
  Deleting a row, column or sheet that another formula still references
  fails; the caller must clear the dependents first.
- **Tables and computed columns.** A table records ordered stable columns,
  case-insensitive unique column names, an optional header row and ordered
  data rows. A computed-column operation stores one formula template against
  a stable `ColumnId` and materialises a compiled formula for every current
  row. A later table-row insertion carries the new row identities, the
  computed cells derived from the template and all structured-range
  rebindings in the same content-addressed event. Application verifies those
  derivations and rolls the entire operation back if any formula fails.
- **Transactions.** `Document::apply` validates an event completely before
  mutating anything. The calculation engine's own transactional cycle
  rejection runs before the core mutation, so an invalid event leaves both the
  state and the digest unchanged.
- **Digest.** `Document::snapshot` is a canonical projection (fixed field
  order, sorted maps) that includes inputs, provenance per cell and computed
  values; `Document::digest` hashes it. Replaying the same events yields the
  same digest.
- **No bypass.** State fields are private; `apply` and `command` are the only
  mutators and both require an actor.
- **No clock.** Timestamps are supplied by the caller and ticks are explicit
  events, so reopening a document can never change a value.

## Consequences

- A1 addresses shown to people are projections (`Document::project_a1`) that
  change when layout changes; the recorded formula source text is the text as
  entered and is not rewritten. Rendering a formula back to current A1 text is
  future work that needs an AST printer in the calculation crate.
- Range references are stored as explicit identity lists, bounded at 10,000
  references per formula. Whole-column semantics remain open. The first
  structured-reference slice covers data, header, all, this-row and inclusive
  column-span selectors; totals-row and escaped-column-name syntax remain
  future compatibility work.
- Branch creation, merge, checks and watched outputs are represented only by
  reserved identity types; their operations belong to the store and safety
  runbook.
- The v0.0.2 LibreOffice path is untouched; the core is a library with no
  persistence and no user interface.

## Alternatives rejected

- **Store the parsed AST in the event.** It would make events depend on the
  engine's private expression type and version; the source text plus ordered
  stable references replays identically and stays readable.
- **Rewrite dependents on delete to `#REF!`.** Excel's behaviour hides
  breakage inside values; refusing the delete keeps the failure explicit and
  transactional, which is what agent edits need.
- **Keep a mutable workbook beside the log.** That would be a second source of
  truth; the document is only ever the replay of its events.
