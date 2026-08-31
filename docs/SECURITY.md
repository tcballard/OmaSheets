# Security model

## Trust boundary

Agent input is untrusted, including tool names, JSON arguments, workbook text,
formulas, names, links, and requested destinations. The same Unix user remains
the administrative boundary: another same-UID process can read user files and
is not cryptographically isolated from OmaSheets state.

## Authority rules

- Agents may read a workbook selected by the local user.
- Agents may submit typed, bounded operations for a selected workbook only
  with evidence from that exact session, revision and semantic source.
- Agents may not supply raw paths, choose replacement mode, approve, commit,
  reject, or undo.
- Local CLI and panel review may approve, reject, commit, and undo.
- `.xlsm` is read-only; `.xls` can only produce a separate `.xlsx`.

## Installation and dependency authority

The Omarchy plugin manager clones, validates and enables the repository but
runs no OmaSheets hooks. The bar widget invokes only fixed argv rooted at the
validated plugin source directory. Bootstrap never runs `sudo` or a package
manager; missing LibreOffice, GTK3, Python UNO and Bubblewrap runtime components
are reported with an explicit `omarchy pkg add` command for the user to approve.
Compilers, CMake, `pkgconf`, and LibreOffice development headers remain confined
to release CI and are not user dependencies.

Product files are user-local. The installer refuses pre-existing unowned target
paths, rewrites the installed Codex MCP command to an absolute owned launcher,
and records hashes for removal. Modified desktop/plugin/launcher files are
preserved, and unrelated MIME or Codex marketplace entries are not rolled back.

Native binaries embed the source commit and full tracked-source digest used by
the release compiler. The installer enforces release checksum, allow-listed
archive contents, version, platform, architecture, exact checkout identity and
per-file hashes before committing application state. Compiler-free CI also
verifies the identity exposed by each installed binary against the checkout and
installation receipt; a detached checksum file alone is not treated as
sufficient provenance.

## Calc isolation

Production jobs require Bubblewrap and run with:

- a new user, PID, IPC, UTS, cgroup, and network namespace;
- a minimal read-only runtime filesystem;
- read-only loader paths or equivalent merged-`/usr` compatibility symlinks;
- read-only NSS identity, machine identity, timezone and fontconfig runtime
  files required for headless LibreOffice bootstrap;
- a private writable job directory and fresh Calc profile;
- no inherited home, SSH agent, cloud credentials, or arbitrary environment;
- macro execution and automatic link/update behavior disabled;
- bounded time, output size, sheet count, cell count, and formula count.

If the required isolation cannot be established, the production worker fails
closed. A clearly labelled development override may exist for tests only.

## Integrity

- Source identity includes a stable regular-file check and SHA-256 digest.
- Plan seals bind the source, revision, normalized operations, staged artifact,
  workflow explanation and cited observations, destination mode, destination
  path, preview, and verification record.
- Observation seals establish which bounded results a plan cited. They do not
  establish that an agent interpreted those results correctly. Agent-authored
  goals, summaries, assumptions and group purposes remain untrusted text.
- Plan revision creates a sealed replacement and marks the previous plan
  superseded under its plan lock; reviewed plan content is never edited in place.
- Receipt records are hash chained and written under a chain lock.
- Plan approval/rejection and publication are serialized by per-plan locks.
- The native diff overlay is derived from sealed verification evidence, capped
  at 200 visible changes, mode `0600`, session/revision bound and presentation
  only. Its approval action uses fixed argv, revalidates live state and can
  publish only to a new, unused same-format destination.
- Copy uses no-clobber publication; replace revalidates while holding a source
  advisory lock and never overwrites unexpected concurrent bytes.

## Known limitations

- LibreOffice and Excel can differ in formulas, layout, names, charts, external
  links, pivot behavior, and unsupported features.
- Full-workbook PDF preview is evidence, not a complete semantic proof.
- Literal search is case-insensitive in v0.0.1; it is not a query language.
- Formula tracing is bounded and cannot resolve every dynamic reference.
- No custom verification scripts run inside the sandbox in v0.0.1.
- Worker failures expose only the bounded structured error written to the
  private job result; process stderr and the inherited environment remain hidden.
- Starting an agent session is an explicit local UI action. OmaSheets passes a
  fixed prompt to `omarchy agent prompt`, so Omarchy—not OmaSheets—selects the
  configured default agent. The prompt contains no workbook path or cell
  content; the selected agent still has the authority of the local user account
  and is outside Calc's networkless worker sandbox.
- The provider-neutral `agent-session` command bridge validates calls against
  the same allowlisted schemas as MCP. It exposes no approval, commit, replace,
  copy-publication or undo operation.
