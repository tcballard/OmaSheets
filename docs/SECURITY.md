# Security model

## Trust boundary

Agent input is untrusted, including tool names, JSON arguments, workbook text,
formulas, names, links, and requested destinations. The same Unix user remains
the administrative boundary: another same-UID process can read user files and
is not cryptographically isolated from OmaSheets state.

## Authority rules

- Agents may read a workbook selected by the local user.
- Agents may submit typed, bounded operations for a selected workbook.
- Agents may not supply raw paths, choose replacement mode, approve, commit,
  reject, or undo.
- Local CLI and panel review may approve, reject, commit, and undo.
- `.xlsm` is read-only; `.xls` can only produce a separate `.xlsx`.

## Calc isolation

Production jobs require Bubblewrap and run with:

- a new user, PID, IPC, UTS, cgroup, and network namespace;
- a minimal read-only runtime filesystem;
- a private writable job directory and fresh Calc profile;
- no inherited home, SSH agent, cloud credentials, or arbitrary environment;
- macro execution and automatic link/update behavior disabled;
- bounded time, output size, sheet count, cell count, and formula count.

If the required isolation cannot be established, the production worker fails
closed. A clearly labelled development override may exist for tests only.

## Integrity

- Source identity includes a stable regular-file check and SHA-256 digest.
- Plan seals bind the source, revision, normalized operations, staged artifact,
  destination mode, destination path, preview, and verification record.
- Receipt records are hash chained and written under a chain lock.
- Plan approval/rejection and publication are serialized by per-plan locks.
- Copy uses no-clobber publication; replace revalidates while holding a source
  advisory lock and never overwrites unexpected concurrent bytes.

## Known limitations

- LibreOffice and Excel can differ in formulas, layout, names, charts, external
  links, pivot behavior, and unsupported features.
- Full-workbook PDF preview is evidence, not a complete semantic proof.
- Literal search is case-insensitive in v0.0.1; it is not a query language.
- Formula tracing is bounded and cannot resolve every dynamic reference.
- No custom verification scripts run inside the sandbox in v0.0.1.
