# ADR-0003: Evolve OmaSheets in place around an event-sourced native core

- Status: Accepted
- Date: 2026-09-01

## Context

The OmaSheets v0.0.2 development baseline establishes an agent-safe product
shell around LibreOfficeKit and isolated Calc workers. It owns the agent entry point,
bounded reads, sealed evidence, typed plans, local approval, verification,
publication receipts and compiler-free Omarchy installation. LibreOffice is
still the workbook model, formula authority and interactive grid.

That baseline is useful, but it cannot by itself deliver a lightweight,
deterministic spreadsheet built for agents. Its operations address sheets and
A1 ranges, history surrounds the workbook rather than describing every edit,
and calculation state belongs to a general-purpose office process. Adding
branches, per-value provenance or safe structural agent edits on top of that
coordinate model would preserve the failure modes the native architecture is
intended to remove.

Creating a second product for the native architecture would also split the
installed identity, safety contracts, compatibility work, users and evidence.
The architectural destination therefore belongs in OmaSheets itself.

This decision refines the engine evolution described by
[`ADR-0001-ENGINE-STRATEGY.md`](ADR-0001-ENGINE-STRATEGY.md) and accepts the
resident-kernel direction proposed by
[`ADR-0002-AGENT-NATIVE-PERFORMANCE.md`](ADR-0002-AGENT-NATIVE-PERFORMANCE.md).
The resource and compatibility gates in ADR-0002 continue to apply.

## Decision

OmaSheets will evolve in this repository, under the existing OmaSheets product
and command identity, toward an event-sourced native document and calculation
core. There will be no parallel repository, binary or product brand for this
work.

The native architecture has the following invariants.

### The document is an event history

For native documents, semantic events are the source of truth. A planned
single-file `.omasheets` format will use SQLite in WAL mode while open and
contain:

- an append-only semantic event log with actor and parent/branch identity;
- stable object IDs for tables, columns, rows, cells or named regions, views,
  checks, branches and watched outputs;
- materialised snapshots so opening a document does not require full replay;
  and
- explicit lineage for imported, calculated and model-assisted values.

Snapshots and columnar batches are performance representations, not competing
sources of truth. Arrow or another columnar representation will be adopted only
where measurement justifies it.

Every mutation, whether produced by a person, import, agent or model-assisted
workflow, must become an attributed event. No native write API may bypass that
log.

### Stable IDs precede native agent writes

A1 notation remains valid at the user interface and compatibility boundaries,
but it is not identity inside the native document. Formulas compile against
stable objects. Insertions, renames and layout changes must not silently
retarget dependencies or permissions.

The current A1-based operation vocabulary remains supported for the v0.0.2
compatibility path. It must not be extended into the native core as a second
mutation model. Before an agent can mutate native content, OmaSheets must have:

1. stable object IDs and deterministic coordinate-to-ID resolution;
2. semantic event operations over those IDs;
3. branch, permission and conflict semantics for those operations; and
4. deterministic projection back to a grid and to compatibility formats.

### Calculation is deterministic and model-free

The native calculation path will contain no LLM or provider call. Formula
parsing, dependency tracking, cycle detection, dirty propagation and
topological recalculation are deterministic engine responsibilities.
Volatile values such as time and randomness require explicit, recorded tick or
seed events; reopening a document must not silently change them.

Model calls remain explicit and asynchronous. Their proposals and accepted
results carry provider, model, parameter, prompt-digest and timestamp
provenance. Models may ingest, organise, audit, explain and propose, but the
deterministic engine calculates and verifies.

### Direct manipulation remains primary

The grid is the primary product surface. **Ask Agent** remains an explicit
entry point and uses the Omarchy user's configured default agent. Natural
language is an input and explanation mechanism, not the stored workbook
representation. There is no notebook execution order or hidden calculation
state; the dependency graph is authoritative.

### Existing safety contracts survive the transition

Native work does not relax the current boundaries:

- agents receive bounded, path-free capabilities rather than ambient file
  authority;
- observations, proposals and engine identities remain sealed and auditable;
- agent changes are staged, reviewed and verified before publication;
- publication and undo remain local-only actions with durable receipts;
- macros, network-capable formulas and external effects remain isolated; and
- large work has explicit limits, cancellation and complete-process evidence.

Branches, checks, permissions and semantic diffs will strengthen this model;
they do not replace the existing human approval boundary.

### LibreOffice becomes a compatibility lane

There is no flag-day engine replacement. LibreOffice remains available for
legacy import, unsupported formulas and objects, rendering, export and
save/reopen comparison while native capabilities earn promotion. It is the
compatibility oracle for a capability until the native implementation passes
the relevant immutable corpus and resource gates.

New native documents use the event-sourced core as authority. Imported
workbooks enter through an explicit conversion event with source provenance.
Export to `.xlsx`, `.ods` or another compatibility format is a projection and
must report features that cannot round-trip. OmaSheets will not maintain a
native event log and a mutable LibreOffice workbook as two co-equal truths.

## Sequence

1. Freeze v0.0.2 as the compatibility and agent-safety baseline.
2. Build the corpus scorer before promoting a native parser or calculation
   engine.
3. Prove the native parser, dependency graph and recalculation path behind a
   non-shipping capability boundary.
4. Introduce stable IDs, semantic events, snapshots and deterministic replay
   before native editing or agent mutation.
5. Add native grid and service projections without removing the compatibility
   lane.
6. Promote import, formulas, edits and objects independently only when their
   correctness, resource and cancellation gates pass.

The milestone gates and stop conditions are recorded in
[`ROADMAP.md`](ROADMAP.md). Passing one workload never promotes unrelated
workloads.

## Consequences

- The current v0.0.2 work remains the foundation rather than disposable
  scaffolding: its authority separation, evidence, review, compatibility and
  packaging contracts are reused.
- The native core is still a substantial engine and document-model project.
  The existing Rust spike is evidence gathering, not proof that the engine or
  storage architecture exists.
- Existing `.xls`, `.xlsx`, `.xlsm` and `.ods` workflows remain useful during
  migration, at the cost of retaining LibreOffice as a dependency until native
  coverage earns its removal for a given workflow.
- The repository may become a mixed Rust, Python, C++ and TypeScript codebase
  during migration. Release artifacts must keep compilers off user machines.
- Coordinate-based compatibility operations will need an explicit translation
  boundary rather than being reused as native identity.
- Performance, formula coverage and compatibility remain measured claims. No
  native-core milestone is implied by accepting this ADR.

This ADR does not move the repository, change the `omasheets` binary or plugin
identity, change the current MIT license, or establish new contribution or
release-governance policies. Any such change requires its own explicit
decision.

## Alternatives rejected

### Build a separate native spreadsheet product

Rejected because it would duplicate the OmaSheets brand, agent protocol,
compatibility lane, packaging and trust model while leaving users to choose
between two incomplete products.

### Keep LibreOffice as the permanent document and calculation authority

Rejected because it cannot satisfy the stable-identity, event-history,
deterministic-provenance and lightweight-residency goals of OmaSheets.

### Replace the current implementation in one rewrite

Rejected because it would discard working safety boundaries and remove the
only compatibility oracle before the native engine has corpus evidence.
