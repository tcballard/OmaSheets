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

The native window publishes a coalesced, private XDG-runtime context record for
the current immutable session. MCP exposes a validated, path-free projection as
`omasheets://window`, joining human selection and viewport state to semantic
agent tools without turning MCP into a remote-control channel.

For workbook content, the window owns a separate bounded Unix-socket bridge.
It uses LibreOfficeKit's save-copy behavior (no `TakeOwnership`) to produce a
new private snapshot in the source format. The isolated UNO worker then applies
the existing semantic read or staging operation to that snapshot. This exposes
unsaved in-memory content without running agent commands in the GTK process or
changing the human view. Snapshot paths remain internal, are mode `0600`, and
are removed after each operation.

After staging, the service derives a mode-`0600`, session-bound review payload
from the sealed plan and its verified target fingerprints. The native window
renders at most 200 exact cell/range changes in a GTK overlay above the
LibreOfficeKit view. Larger proposals retain their total count and visibly say
that the list is truncated. The overlay never sends edit commands to the
document engine.

### Local approval surface

The Omarchy panel can launch a local terminal review, while the native window
offers an explicit **Approve & Save a Copy** confirmation from the diff overlay.
Both paths recheck the plan seal, live semantic fingerprint, revision, staged
hash and destination immediately before publication. Native live-window review
is always copy-only and refuses an existing destination.

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
