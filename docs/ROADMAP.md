# OmaSheets roadmap

This roadmap converges the native, event-sourced spreadsheet architecture into
OmaSheets without creating a second product. It separates what the v0.0.2
baseline actually provides from future targets. Dates and focused-week
estimates are planning aids, not delivery promises.

No milestone is complete merely because code exists for a demo. Promotion
requires reproducible correctness, resource, cancellation and packaging
evidence against immutable fixtures. LibreOffice remains the compatibility
lane until a native capability passes its own gate.

## v0.0.2 — compatibility and agent-safety baseline

After its release gates pass, v0.0.2 establishes the contracts future native
work must preserve:

- an OmaSheets-owned GTK and LibreOfficeKit window with **Ask Agent** as the
  explicit entry point for the user's configured Omarchy agent;
- path-free MCP and provider-neutral command surfaces for bounded workbook
  reads, audits, formula tracing and typed change proposals;
- immutable live snapshots, evidence IDs, sealed and revisable plans,
  recalculation, save/reopen/render verification, local approval, publication
  receipts and bounded undo;
- isolated LibreOffice workers for calculation, import, conversion, rendering,
  charts, pivots and compatibility-sensitive verification;
- batched reads, streaming semantic fingerprints, pre-materialisation limits
  and event-driven native-overlay updates;
- deterministic dense, sparse and formula fixtures plus complete Linux process
  tree RSS, PSS and USS measurement; and
- source-bound, compiler-free native release bundles and an automated Arch
  installation/acceptance path.

These are baseline capabilities, not evidence of a native calculation engine.
In particular, v0.0.2 does **not** establish:

- a native `.omasheets` event store or stable object identities;
- a native formula parser, dependency graph or incremental recalculation;
- per-value provenance inside the document;
- persistent document branches, semantic merge or first-class checks;
- native decimal currency, units or probability distributions;
- a million-row native grid or the latency and idle-memory targets below; or
- corpus-scale Excel formula and round-trip parity.

The isolated Rust experiment under `spikes/` remains a spike. Its results do
not satisfy M0.

## Shared gates

Every milestone must retain all of the following:

1. Agents cannot publish, approve or gain arbitrary filesystem authority.
2. Every promoted native result identifies its engine and can be compared with
   an immutable compatibility result.
3. Large work is bounded before materialisation and has a tested cancellation
   path.
4. Compiler-free release artifacts are tied to exact tracked source and pass
   the installed-product suite.
5. Existing compatibility workflows do not silently change authority.
6. Performance numbers state the fixture, hardware, cold/warm state, process
   boundary and exact metric measured.

## M0 — native engine and corpus spike

**Purpose:** decide whether OmaSheets should own its calculation hot path.

**Build:**

- Add a Rust workspace inside this repository for core types, formula parsing,
  dependency graphs and calculation experiments.
- Fetch rather than vendor representative Enron and EUSES workbook samples,
  then build the open/parse/recalculate scorer before selecting an engine.
- Parse an Excel-compatible syntax subset, construct a dependency graph,
  detect cycles and recalculate dirty transitive closures in topological order.
- Implement approximately 40 high-frequency functions with explicit error and
  volatile-value semantics.
- Import `.xlsx` through Calamine, retaining the source workbook and recording
  unsupported features rather than silently accepting them.
- Benchmark synthetic 100,000-formula and one-million-formula graphs plus real
  corpus workbooks. Keep parsing, import, recalculation and process startup as
  separate measurements.
- Compare candidate libraries with a minimal owned implementation; do not
  promote a dependency solely from a toy-workbook timing.

**Exit gate:** a written go/no-go review with reproducible corpus scores and
performance distributions. The target is incremental recalculation below
10 ms p95 for one edit in a 100,000-formula document and full recalculation of
one million simple formulas below 1.5 seconds on the declared eight-core x86
baseline. If incremental recalculation remains above 25 ms p95 after one
focused rearchitecture attempt, stop or rescope the native-engine thesis.

**Not delivered by M0:** a user-editable native document, a production grid or
a replacement for the v0.0.2 compatibility path.

## M1 — event-sourced local alpha

**Purpose:** make the native model usable for real local work without giving it
public compatibility claims.

**Build:**

- Introduce stable IDs, semantic events, deterministic replay and a single-file
  SQLite `.omasheets` document with bounded materialised snapshots.
- Model tables, columns, rows, named cells or ranges, freeform regions and
  sheet views without using grid coordinates as identity.
- Record human edits, imports, explicit ticks and accepted model-assisted
  proposals with actor and lineage metadata.
- Grow the formula surface toward approximately 120 measured functions,
  including structured references and computed columns.
- Add inferred base column types without silently coercing mixed data.
- Expose one loopback-only local service with per-session authentication and a
  CLI over the same API.
- Spike a keyboard-first virtualised grid, including provenance indicators,
  formula-range overlays, half-tile layouts and full mouse-optional operation.
  Qt 6, Qt Quick and CXX-Qt are the selected spike candidate, not accepted
  dependencies until the customization and accessibility review in
  `docs/M1-GRID-SPIKE.md` passes.
- Keep the current LibreOfficeKit window available for compatibility documents
  while the native grid is incomplete.

**Exit gate:** the maintainer uses a native document for at least one recurring
dogfood workload, every native edit replays bit-identically, and the grid spike
passes its customization/accessibility review. Target measurements, not current
claims, are:

| Metric | Target on the declared baseline |
| --- | ---: |
| Cold open, 10 MB / 100,000-cell native document | < 300 ms p95 |
| Keystroke to committed paint | < 16 ms p95 |
| Sustained scroll, one-million-row table | 60 fps |
| Idle RSS, 100,000-cell native document | < 150 MB |

Missing a target records a blocker and evidence; it does not get rounded into a
pass.

## M2 — safe semantic workflows and first native public release

**Purpose:** make native documents reviewable, scriptable and safe for public
use before adding model-led automation.

**Build:**

- Add named event-log branches, watched outputs, semantic diffs and
  operation-level merge conflicts.
- Make checks first-class deterministic graph nodes with severity and messages;
  `omasheets check` exits non-zero for configured failures.
- Require checks to pass before branch merge while retaining explicit local
  human approval.
- Add import and export manifests. Export CSV and Parquet natively and project
  the supported value, format and formula subset to `.xlsx`; disclose every
  native feature that cannot round-trip.
- Consume the active Omarchy theme, retain loopback/session-token hygiene and
  add an explicit `omasheets setup --omarchy` path for optional user-service,
  web-app and file-association setup.
- Prepare source and binary AUR packages only after the installed-product gates
  are reproducible.

**Exit gate:** branches, checks, semantic diff and export are exercised through
the same CLI/API used by the UI. On a frozen 1,000-workbook corpus sample, the
targets are at least 99% open-without-crash, 97% formula parse coverage and 95%
matching stored results among cells that can validly be recalculated, using a
declared relative-error policy. These are targets; no present OmaSheets result
is counted toward them until the corpus and scorer are checked in.

A native public release also requires the applicable maintainer communication,
license, contribution and distribution decisions to be recorded separately.
This roadmap does not silently change the repository's current governance.

## M3 — librarian, auditor, ingestion and native agent operations

**Purpose:** let agents improve native documents without becoming calculation
or publication authorities.

**Build:**

- Add deterministic table detection, formula normalization, inconsistent-copy
  detection, hardcoded-override detection, edge-reference checks and stale
  import checks before adding model triage.
- Add reviewable ingestion, librarian and natural-language-to-formula
  proposals. Accept/reject decisions become semantic events; no generated
  value silently enters the calculation graph.
- Keep **Ask Agent** and the configured Omarchy agent as the primary routing
  contract. Do not embed a provider-specific SDK in the engine; any direct
  model endpoint used for bounded structuring work remains optional and
  replaceable.
- Replace coordinate mutation for native documents with MCP tools over stable
  IDs: overview, table query, provenance, explain, branch creation, proposed
  semantic edits, checks, diff, gated merge and export.
- Store per-agent table/object permissions in the document. Agents work on
  branches by default, cannot merge without policy permission, and never gain
  publication authority through MCP.
- Explain selected values through deterministic dependency and lineage facts;
  a model may phrase those facts but cannot invent the chain.

**Exit gate:** an agent can build a checked model on a branch, present a
deterministic semantic diff and hand it to local human approval without
corrupting the main branch. AI quality is measured on an early hand-labelled
corpus rather than anecdotes. Initial targets are table-bound detection
F1 >= 0.90, header accuracy >= 0.95, type accuracy >= 0.92, and seeded-audit
precision >= 0.80 at recall >= 0.50. Fully local operation must remain
functional and its quality gap must be reported.

## M4 — exact money, units, uncertainty and secondary surfaces

**Purpose:** add developer-shaped modelling capabilities after the native
document, calculation and safety model is stable.

**Build:**

- Add exact decimal arithmetic for currency columns while retaining documented
  IEEE-754 behaviour for ordinary numbers.
- Add typed dates and timezone-aware timestamps, plus column-level units and
  compile-time dimensional checks. No implicit currency conversion is allowed.
- Add seeded distributions and deterministic Monte Carlo propagation with the
  seed and sampling policy stored as document state.
- Render native line, bar and sparkline charts, including compact distribution
  summaries. Existing LibreOffice-backed charts remain compatibility features
  until native equivalents pass their own gates.
- Add a read-only terminal viewer after the primary grid and CLI are proven.

**Exit gate:** incompatible units fail formula compilation with an actionable
diagnostic; currency test vectors are exact; stochastic outputs reproduce
bit-identically for a fixed engine version, seed and policy; and native charts
render from the same event snapshot used by checks and agent explanations.

## Explicitly parked

The following do not enter M0-M4 without a separate decision supported by
dogfood or user evidence:

- collaborative editing or CRDTs;
- streaming/event-time tables and historical as-of views;
- full Excel function, macro or pivot-table parity;
- LAMBDA completeness;
- a LibreOffice source fork; and
- platform expansion that delays the Omarchy/Linux product path.

## Product stop conditions

Stop, freeze or rescope rather than carrying a failed premise indefinitely:

1. Apply the M0 recalculation stop condition after its single permitted
   rearchitecture attempt.
2. If corpus open-without-crash remains below 90% after two focused import fix
   cycles, either stop compatibility expansion or explicitly scope native
   documents as the product and `.xlsx` as best-effort import.
3. If OmaSheets is not used weekly for a real recurring workflow by the M3
   exit review, complete safety-critical work, feature-freeze and reassess.
4. Define adoption gates before the M2 native public release; do not invent
   them after observing launch results.

The next implementation action is M0's corpus scorer, not a wholesale rewrite
of the v0.0.2 product.
