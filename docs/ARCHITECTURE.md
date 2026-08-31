# v0.0.1 architecture

OmaSheets deliberately separates spreadsheet calculation, agent planning, and
filesystem publication.

OmaSheets is not currently a LibreOffice source fork. It owns the Omarchy and
agent-facing product layer while LibreOffice Calc is a replaceable document
engine. The accepted rationale and native-shell evolution path are in
[`ADR-0001-ENGINE-STRATEGY.md`](ADR-0001-ENGINE-STRATEGY.md).

## Components

### Calc worker

A short-lived LibreOffice Calc/UNO process is the authority for importing,
calculating, saving, reopening, inspecting, and rendering workbooks. Each job
uses a fresh profile, a private UNO pipe, no network namespace, no home-directory
mount, macro execution disabled, and document-update prompts disabled.

### OmaSheets service

The service owns workbook sessions, immutable source hashes, staged plans,
semantic diffs, verification records, plan seals, publication, receipts, crash
recovery, and undo. It never treats an agent-supplied path as authoritative.

### MCP server

The MCP server exposes bounded read operations and change planning. Tool schemas
reject unknown arguments. Hidden implementation arguments cannot be smuggled in
by a client. `apply_plan` is a read-only handoff that returns local review
instructions; it does not commit.

### Local approval surface

The Omarchy panel launches a local terminal review. Approval requires an exact
token containing the plan identifier. The review surface rechecks the plan seal,
revision, hashes, and preview immediately before publication.

## State model

A plan progresses through:

`planned -> verified -> approved -> committed`

It may instead become `rejected`, `conflicted`, or `failed`. `approved` is a
durable recovery state, not evidence that publication completed. A durable
receipt is the commit linearization point.

## Publication modes

- **copy**: publish to a new destination using no-clobber semantics.
- **replace**: local-only, explicit approval; lock and revalidate the source,
  create a verified backup, atomically replace, fsync, and record undo metadata.

Unexpected bytes at a destination are preserved. OmaSheets never deletes or
restores over a concurrent writer merely to make its own transaction succeed.

## Calculation and verification

LibreOffice Calc is the calculation authority. A verified staged artifact is
recalculated, saved, reopened, inspected for formula errors, and rendered. The
service compares sheet inventory, used ranges, names, and bounded formula
records when available. These checks detect important regressions but do not
prove Microsoft Excel equivalence.
