# v0.0.1 architecture

OmaSheets deliberately separates spreadsheet calculation, agent planning, and
filesystem publication.

OmaSheets is not currently a LibreOffice source fork. It owns the Omarchy and
agent-facing product layer while LibreOffice Calc is a replaceable document
engine. The accepted rationale and native-shell evolution path are in
[`ADR-0001-ENGINE-STRATEGY.md`](ADR-0001-ENGINE-STRATEGY.md).

## Components

### Installation boundary

Omarchy `plugin add` installs and enables only the validated repository checkout;
the official lifecycle has no install or uninstall hooks. `Panel.qml` therefore
uses `manifest.__sourceDir` to invoke a fixed-argv repository-local bootstrap.
The user explicitly starts that bootstrap from the widget or terminal.

The bootstrap checks external Arch dependencies without installing them, builds
`native/libreofficekit` from the checkout, and installs the Python package,
native binaries, stable launcher, Codex plugin/MCP configuration, desktop entry
and MIME associations together. Application bytes live below `XDG_DATA_HOME`,
build output below `XDG_CACHE_HOME`, journals below `XDG_STATE_HOME`, and live
sockets/snapshots below `XDG_RUNTIME_DIR`.

An installation journal records exact hashes and previous shared-file content.
Uninstall restores unchanged shared files, surgically removes OmaSheets entries
from concurrently edited MIME/marketplace files, and preserves modified owned
files as explicit conflicts.

The installer hashes the complete tracked source set and passes that identity
plus the Git commit into the native compiler. Both binaries expose the embedded
identity through `--provenance`; Arch CI compares it with the installed checkout
and the installation receipt.

### Native window

The OmaSheets-owned GTK3 process embeds LibreOfficeKitGTK. It is installed from
the production native source boundary and is the desktop/MIME launch target.
LibreOfficeKit remains replaceable and its unstable API does not change the
agent or publication authority boundaries.

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

`omasheets://session` is the path-free entry point for a new workbook task. The
native window and Omarchy panel invoke the owned `agent-session` command with no
agent name or user-controlled arguments. That command gives a fixed product
prompt to `omarchy agent prompt`, which resolves the user's configured default
agent. The prompt contains no workbook path or content. The selected agent
obtains the session and live focus through MCP, then uses ordinary bounded tools.
If that agent does not discover the installed MCP server, the fixed prompt
directs it to the `omasheets agent-session` JSON command bridge. That bridge
uses the same tool schemas and service methods and likewise exposes no commit or
publication operation.

Successful reads create sealed observation records below the private XDG state
directory. Each record binds the session, revision, exact selected-file or
live-window semantic source, tool arguments and result digest. Agent plans cite
those record IDs and carry bounded goal, summary, assumptions and purpose
groups. These explanations are sealed and presented, but are not calculation
evidence. `revise_plan` stages a complete replacement from the same immutable
base and marks its predecessor `superseded`; it never edits a reviewed plan in
place.

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
LibreOfficeKit view. The overlay also presents the sealed goal, summary,
assumptions and operation-group purposes. Larger proposals retain their total count and visibly say
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

It may instead become `superseded`, `rejected`, `conflicted`, or `failed`. `approved` is a
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
