# LibreOfficeKit native-rendering spike

This spike now contains both the document-engine proof and the first real
OmaSheets-owned workbook window. The window embeds LibreOfficeKitGTK's
interactive tiled view inside OmaSheets chrome; it is not a re-skinned Calc
window.

It is deliberately not a product editor. LibreOfficeKit's tiled rendering and
editing API is unstable, and this process must remain isolated from the future
OmaSheets shell.

## Build on Omarchy

Arch's `libreoffice-fresh` package supplies the runtime and
`libreoffice-fresh-sdk` supplies the headers used by the spike.

```bash
sudo pacman -S --needed cmake gtk3 libreoffice-fresh libreoffice-fresh-sdk
cmake -S spikes/libreofficekit -B build/lok-spike
cmake --build build/lok-spike
build/lok-spike/omasheets-lok-render workbook.xls /tmp/omasheets-tile.ppm
build/lok-spike/omasheets-window workbook.xls
sudo cmake --install build/lok-spike
omasheets lok status
omasheets lok render workbook.xls --output /tmp/omasheets-cli-tile.ppm
```

Set `OMASHEETS_LOK_PROGRAM` only when LibreOffice's `program` directory is not
`/usr/lib/libreoffice/program`. Each invocation creates a new temporary user
profile and removes it on exit; the helper never reuses the person's normal
LibreOffice profile.
The output path must be new: both the native helper and Python launcher refuse
to replace an existing file.

## Interactive window

`omasheets-window` provides:

- horizontal and vertical scrolling with visible-area updates sent to the
  engine;
- mouse selection and direct keyboard editing through `LOKDocView`;
- live cell address and formula/value display;
- sheet switching, zoom, undo/redo, copy/paste, bold and italic actions;
- a session-bound agent diff overlay with verified before/after cards,
  destructive/truncation warnings, dismissal and explicit copy-only approval;
- a human-only **Save a Copy** flow which refuses to replace an existing path;
- an isolated temporary LibreOffice profile for every window;
- dirty-close confirmation and password prompts; and
- a clear status/error surface owned by OmaSheets.

Scroll and resize bursts are coalesced to one visible-area update per 16 ms
frame. CI opens a sparse 100,000-row by 50-column workbook, scrolls deep into
it, captures the painted window, and enforces first-paint, load-time, memory,
and viewport-update budgets. This prevents v0.0.1 from quietly regressing into
full-sheet rendering or one engine update per input event.

The GTK widget is an engine-facing component. Window layout, controls, product
identity, save policy, profile isolation, and launch authority belong to
OmaSheets.

The PR smoke test runs in Arch Linux, creates a genuine Excel 97 `.xls` from
[`fixtures/smoke.fods`](fixtures/smoke.fods), loads it through LibreOfficeKit,
renders a 320×200 tile, and validates the output header and byte count.

## What this answers

| Question | Spike evidence |
| --- | --- |
| Can Omarchy package the headers and runtime? | `libreoffice-fresh` and `libreoffice-fresh-sdk` expose both through official Arch packages. |
| Can OmaSheets initialize without UNO? | The helper calls `lok::lok_cpp_init` directly. |
| Can it recognize a Calc document? | Non-spreadsheet document types are rejected. |
| Can it render into an OmaSheets-owned buffer? | `paintTile` fills a bounded RGBA/BGRA buffer which is exported as PPM. |
| Is the user's LibreOffice profile reused? | No; every helper process receives a one-shot profile URL. |
| Is there an OmaSheets-owned window? | Yes. CI launches it under Xvfb and captures the rendered workbook window. |
| Are scrolling and selection wired? | Yes. GTK scroll adjustments update the LOK visible area; LOKDocView owns pointer selection and keyboard editing. |
| Does a large sparse sheet stay bounded? | CI enforces a 15 s load, 5 s first-paint, 1 GiB RSS ceiling and coalesced viewport updates on 100,000 × 50 used dimensions. |
| Is this a complete native editor? | No. Accessibility evidence, complex dialogs, crash sandboxing, and Wayland acceptance remain open. |

## Promotion gate

Do not move this helper into the default `omasheets open` path until a native
shell proves all of the remaining items in
[`../../docs/ACCEPTANCE.md`](../../docs/ACCEPTANCE.md), including interactive
input, tile invalidation, accessibility, latency, crash recovery, and safe
document publication.
