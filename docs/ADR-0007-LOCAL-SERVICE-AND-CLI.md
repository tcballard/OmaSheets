# ADR-0007: One local API, served over a private socket

- Status: Proposed (first slice landed as `omasheets-service`)
- Date: 2026-09-02

## Context

M1 in [`ROADMAP.md`](ROADMAP.md) asks for one loopback-only local service
with per-session authentication and a CLI over the same API. Until now the
native document could be driven only by linking `omasheets-store` or by the
`omasheets-store` command, which exposes primitives rather than an API a
grid, an agent bridge and a person's shell can share.

## Decision

`crates/omasheets-service` defines the API once, as data:

- **`Request` and `Response` are plain serialisable enums.** Every request
  names the document by path, so one service can hold several documents
  open. The requests are: create, open, close, document summary, a bounded
  page of cells, one cell, lineage, append a command, branch, check, diff,
  merge and snapshot. `Command` from the event core is serialisable so an
  append carries the same command the library takes.
- **`Service` is the in-process form.** The CLI, tests and any embedding
  call `Service::handle` directly; the socket server wraps the same struct
  behind a mutex. Errors are `ServiceError { code, message, details }` with
  stable codes (`unknown_branch`, `agent_on_main`, `checks_failed`,
  `conflicts`, `unauthorized`, …) so clients branch on codes, not prose.
- **Authority rules live in the service, not in clients.** An agent actor
  may not append to `main`; it works on a branch and a human merges. Only a
  human actor may approve a merge. Every refusal happens before anything is
  written. Publication (writing an `.xlsx`, updating the installed product's
  document) is not in this API and stays behind the v0.0.2 approval flow.
- **Transport is a Unix socket in the user's runtime directory.**
  `omasheets-service serve` listens on `$XDG_RUNTIME_DIR/omasheets/native.sock`
  in a directory that must be mode 0700, writes a fresh random 32-byte
  session token to `native.token` (mode 0600), and closes any connection
  whose first line is not that token. Requests and answers are one JSON
  line each. A Unix socket is stricter than a loopback TCP port: nothing on
  the network can reach it, and the kernel enforces the ownership check.
- **The CLI is the client.** `omasheets-service call REQUEST_JSON` reads
  the token, sends one request and prints the envelope; exit status 2 means
  the service refused, 1 means the client could not ask.

## Consequences

- One request handler serves the shell, tests and the future grid. Adding a
  request means adding one variant and one match arm, and the JSON shape
  is documented by the types.
- Request lines are capped at 4 MiB and cell pages at 10,000 cells, so a
  client cannot exhaust the service; long documents page.
- The service is single-threaded behind a mutex. That is deliberate at M1:
  correctness and evidence first, concurrency when a workload needs it.
- The grid spike may need a browser-reachable transport. That would be an
  adapter over the same `Service`, decided with the grid, not a second API.

## Alternatives rejected

- **HTTP on 127.0.0.1 now.** Reachable by every local process and browser
  tab; a token would be the only defence. The socket's directory permissions
  are a stronger, simpler boundary, and an HTTP adapter can come later.
- **Extending the `omasheets-store` CLI.** It would keep growing a second,
  primitive-shaped surface. It stays for replay and store-level evidence.
- **Letting clients enforce agent rules.** Every client would have to get
  them right; the service refusing is the only place the rule cannot be
  skipped.
