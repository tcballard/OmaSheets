# ADR-0006: One SQLite file, an authoritative event log, and gated branch merges

- Status: Proposed (first slice landed as `omasheets-store`; not yet the installed product store)
- Date: 2026-09-01

## Context

[`ADR-0005-STABLE-ID-EVENT-CORE.md`](ADR-0005-STABLE-ID-EVENT-CORE.md) makes a
document the replay of a content-addressed event log held in memory. A
product needs that log on disk in one file that survives interruption, opens
quickly after thousands of events, records where every value came from, and
lets an agent work on a branch that a person merges only after deterministic
checks pass.

## Decision

`crates/omasheets-store` keeps one `.omasheets` file per document as a SQLite
database:

- **The event log is the only source of truth.** Events are inserted with
  their canonical JSON, verified against their content address on insert, in
  `IMMEDIATE` transactions with `journal_mode = WAL` and `synchronous = FULL`
  while the file is open. An event that `append` returned is on disk;
  an event the core rejects is never written. State tables do not exist.
- **Schema versioning.** A `meta` table records `schema_version` and a
  format marker. Opening an older file migrates it forward in place
  (version 0 gains the `(branch, seq)` index); a newer or foreign file is
  refused with an explicit error.
- **Snapshots are caches.** Every N events (default 100) and on `close`,
  the branch head's canonical snapshot and digest are stored. On open, the
  document rebuilt from the newest snapshot is used only when its digest
  matches the recorded digest and its head matches the log; otherwise the
  store replays the log and reports `snapshot_rejected`. Snapshot recovery
  can never write to the log.
- **Lineage.** Every cell carries the event, actor and timestamp that set it;
  formula cells additionally expose the cells they read. `Document::lineage`
  classifies values as entered, imported, computed, agent, model-assisted or
  system by actor kind.
- **Branches.** A branch is forked by the first event carrying the new branch
  identity, with the fork point as its parent. A branch's document is the
  replay of its ancestry up to each fork point plus its own events. Branch
  identities derive from the fork head, so replay mints them again.
- **Checks and watched outputs** are first-class objects: a check names a
  cell and passes only when that cell is exactly `TRUE`; checks run through
  the calculation graph like any formula. Watched outputs name cells whose
  value changes a diff must report.
- **Diff** between a source branch and a target since their last common
  point lists the semantic operations on each side, the watched-output values
  before and after, the objects both sides touched, and the source's check
  results. It never exposes SQLite rows.
- **Conflicts are operation-level.** Two sides conflict when their touch
  sets intersect (a cell, a row, a column, a sheet, a table, a check, a watch,
  a proposal). Conflicts block a merge until a person resolves them with
  explicit events.
- **Merge is gated.** `merge` requires a human approver, no error-severity
  check failing on the source, and no conflicts. It replays the source's
  unmerged operations onto the target as new events attributed to their
  original actors, appends a `RecordMerge` event, and commits all of it in
  one transaction; repeated merges see only the source's newer events.
  Agents can append on their own branch and cannot merge or publish.
- **CLI.** `omasheets-store replay|check|branch|diff|merge` wrap the same
  library calls; `check` exits non-zero when an error-severity check fails,
  and `merge` names a human approver on the command line.

## Consequences

- Automatic checkpoints count events since the previous checkpoint, including
  batches that cross the threshold. Only the latest checkpoint per branch is
  retained. Snapshot loads query the verified boundary event and subsequent
  events through each ancestry fork bound; older canonical event payloads are
  not parsed. Invalid checkpoints still fall back to full replay. A grid window
  requests a final snapshot before its launcher stops an owned service, without
  closing a store shared with other windows.

- Durability is SQLite's: committed events survive a killed process; WAL
  files sit beside the document until `close` checkpoints them.
- Loading a long branch costs one snapshot rebuild plus the tail; corrupt or
  stale snapshots cost a full replay, never wrong state.
- A merge does not preserve the source's event identities; it records them.
  Cherry-picking and rebasing are the same operation from the store's view.
- Conflict resolution is manual by design: the store refuses, it does not
  guess. Automatic resolution policies belong to a later decision.
- Publication (writing an `.xlsx` or updating the installed product's
  document) is outside this crate and remains behind the v0.0.2 approval flow.

## Alternatives rejected

- **Mutable state tables beside the log.** A second source of truth that
  would drift from the log and let a bug persist state no event explains.
- **Snapshots as authority after N events.** Would let a corrupt or stale
  snapshot replace history; here a snapshot is discarded the moment its
  digest disagrees.
- **Model-judged merges.** Merge safety rests on deterministic checks and
  conflict sets; a model may propose events, never decide a merge.
