# Rust application migration: required before v0.1.0

Tom required removal of Python on 30 September 2026. The preview release is
paused until the migration is complete. Qt/QML and the LibreOfficeKit C/C++
boundary remain; the application backend and orchestration move to Rust.
No compatibility feature, approval gate or rollback guarantee may be removed
merely to make the dependency list shorter.

## Current implementation

`crates/omasheets-app` adds a Rust `omasheets` binary with native launch/service
supervision and the five bounded native agent tools over CLI and MCP. It uses
private owned session/token files, bounded socket messages, stale-session checks,
path-redacted errors and no automatic write retries. Shared window leases keep
the owned service alive until the last grid closes. It is **not yet installed
by the production package**, and it does not fall back to Python.

Build the Rust migration independently:

```bash
cargo build --locked -p omasheets-app -p omasheets-service
cargo test --locked -p omasheets-app
cargo clippy --locked -p omasheets-app --all-targets -- -D warnings
cargo build --locked -p omasheets-app --examples
target/debug/examples/native_acceptance
```

The acceptance binary uses the real local service and a small Rust grid stand-in
for multi-window lifecycle testing. It is not visual Qt/Omarchy acceptance. The
agent subprocesses run with an empty executable search path. No Python process
participates in these tests.

## Remaining migration work

| Responsibility | Existing Python modules | Required replacement/parity |
| --- | --- | --- |
| Compatibility workbook operations | calc_engine, calc_worker, lok_spike | Rust orchestration and native LibreOffice interface; inspection, audit, formulas, styles, charts, pivots, conversion, render/reopen evidence |
| Compatibility review and publication | service, transactions, operations, policy, workflow, store, identity | Stable identities, bounded typed proposals, immutable source checks, human approval, no-clobber publication, backup and undo |
| Compatibility live UI | live_bridge, diff_overlay, native_window | Same selection/snapshot/review contracts at the existing C++ window boundary |
| Complete CLI and MCP | cli, mcp, agent_session | Preserve compatibility resources and tools alongside native tools; never expose approve/commit to agents |
| Installation and trust | installation, native_bundle, release_signing, integration, user_service, doctor, package_install | Rust installer, signature/provenance verification, reversible migration/integration, package-owned file protection and accurate diagnostics |
| Build/release/tooling | scripts/*.py, tests/*.py, pyproject.toml | Rust or standard build tools; retain independent format validation and security regression evidence |
| Package/runtime | packaging/arch, bin, Panel.qml, native/setup, Qt launch calls | Ship Rust launcher; remove Python/UNO startup and all Python package payloads; validate real installed paths |

## Completion gate

- Inventory every tracked Python file and interpreter invocation; port each
  responsibility or explicitly identify a retained third-party dependency.
- Run equivalent safety and feature tests against the Rust implementation.
- Remove the Python source/package, executable calls and dependency declaration.
- Build and install in a clean Arch runtime with no Python interpreter; exercise
  native and compatibility documents, agent review, install/upgrade/migration,
  export/reopen, uninstall and rollback.
- Verify `pacman -Ql`, dependency closure and launched child processes. A clean
  Rust binary alone is insufficient if an installed helper still needs Python.
- Re-run corpus and real Omarchy acceptance against the final source before
  resuming v0.1.0 preparation. Release signing and corpus-scope decisions still apply.

## Verification of the first migration step

On 30 September 2026, using Rust 1.88.0 in the execution workspace:

- Rust app compilation and six schema, MCP framing and private-file boundary tests pass.
- The real native service compiles with debug information disabled and one codegen unit,
  after removing corrupted zero-length local compiler artifacts.
- Four socket transport tests and the real-service acceptance run are **blocked locally**:
  the environment refuses Unix socket creation with `Operation not permitted`.
  These tests remain mandatory in CI; they are not skipped or represented as passing.
- No real Qt window or Omarchy desktop acceptance has been performed for this step.

The release workflow explicitly blocks on tracked `.py` files or `pyproject.toml`
until the remaining implementation and tooling have been replaced.
