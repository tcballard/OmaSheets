# LibreOfficeKit native-rendering spike

This spike proves the document-engine half of an OmaSheets-owned workbook
window. It loads a spreadsheet through LibreOfficeKit, initializes the tiled
renderer, paints the first viewport into memory, and writes a dependency-free
PPM image for inspection.

It is deliberately not a product editor. LibreOfficeKit's tiled rendering and
editing API is unstable, and this process must remain isolated from the future
OmaSheets shell.

## Build on Omarchy

Arch's `libreoffice-fresh` package supplies the runtime and
`libreoffice-fresh-sdk` supplies the headers used by the spike.

```bash
sudo pacman -S --needed cmake libreoffice-fresh libreoffice-fresh-sdk
cmake -S spikes/libreofficekit -B build/lok-spike
cmake --build build/lok-spike
build/lok-spike/omasheets-lok-render workbook.xls /tmp/omasheets-tile.ppm
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
| Is this a complete native editor? | No. Input events, callbacks, accessibility, scrolling, selection, formula editing, and crash sandboxing remain open. |

## Promotion gate

Do not move this helper into the default `omasheets open` path until a native
shell proves all of the remaining items in
[`../../docs/ACCEPTANCE.md`](../../docs/ACCEPTANCE.md), including interactive
input, tile invalidation, accessibility, latency, crash recovery, and safe
document publication.
