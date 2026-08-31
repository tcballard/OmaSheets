# ADR-0001: Own the product shell, not a LibreOffice fork

- Status: Accepted
- Date: 2026-08-31

## Context

OmaSheets intends to replace the everyday Microsoft Excel dependency for an
Omarchy user. The phrase "Oma-version of LibreOffice" can describe two very
different projects:

1. rebrand and maintain a source fork of the complete LibreOffice desktop; or
2. build an OmaSheets-native product and agent surface while using LibreOffice
   as a replaceable document engine.

The v0.0.1 implementation is the second. It currently opens human work in the
standard Calc desktop and runs separate headless Calc/UNO jobs for bounded agent
inspection, recalculation, rendering, conversion, and staging.

## Decision

OmaSheets owns:

- the Omarchy-native application identity and launch integration;
- the future workbook shell, command model, review experience, and visual
  language;
- the typed agent protocol and its authority separation;
- plan verification, publication, receipts, crash recovery, and undo;
- compatibility evidence and the decision to accept or reject engine output.

LibreOffice Calc is a replaceable engine adapter. It is not the OmaSheets
product identity and OmaSheets does not claim to be a LibreOffice fork.

The next native-editor investigation should use LibreOfficeKit behind a narrow
process boundary. LibreOfficeKit offers direct C/C++ access without UNO and an
experimental tiled rendering/editing interface. Because the tiled API is
explicitly unstable, v0.0.1 does not depend on it and a spike must prove
packaging, crash containment, input handling, accessibility, rendering latency,
and spreadsheet-specific editing before it becomes the main UI path.

## Why not fork now

A full fork would make OmaSheets responsible for a very large upstream desktop,
its import/export filters, formula engine, UI toolkit, security fixes,
localisation, accessibility, and release cadence before the distinctive product
surface exists. That would slow the agent-native work and create a permanent
merge burden without yet improving workbook semantics.

## Evolution path

1. **v0.0.1 — integrated engine:** Calc desktop for people; isolated UNO jobs
   for agents; OmaSheets owns selection, planning, verification, and publication.
2. **v0.1 investigation — native shell:** prototype a Quickshell/GTK or Qt
   workbook window backed by an isolated LibreOfficeKit renderer/editor.
3. **Later — engine competition:** keep the adapter contract narrow enough to
   compare LibreOffice, a purpose-built grid/model, or another engine using the
   same compatibility corpus and receipts.
4. **Fork only with evidence:** consider an upstream source fork only if a
   required user-visible capability cannot be delivered through supported or
   containable engine interfaces and the maintenance cost is explicitly funded.

## Consequences

- The current PR is honestly described as an OmaSheets layer, not a LibreOffice
  fork or a complete native spreadsheet editor.
- Native product differentiation can advance without changing calculation and
  file-format authority at the same time.
- The engine boundary must remain observable in capabilities and receipts.
- Compatibility claims remain evidence-based and never imply Excel equivalence.

## Primary reference

- LibreOfficeKit API documentation: https://docs.libreoffice.org/libreofficekit.html
