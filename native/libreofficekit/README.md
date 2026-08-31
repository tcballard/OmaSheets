# OmaSheets native LibreOfficeKit engine

This directory is the production source boundary for the OmaSheets-owned GTK3
window and bounded LibreOfficeKit renderer. LibreOfficeKit remains a replaceable
document engine; the window chrome, session bridge, review overlay, save-copy
policy and launch authority belong to OmaSheets.

Users do not build or install this directory manually. Release CI builds these
targets once from the tagged checkout and publishes a source-bound native
bundle. The repository-local bootstrap verifies that bundle and installs it
into the current user's XDG data directory. It never invokes a compiler,
package manager, or sudo.

Each binary embeds the Git commit and full tracked-source digest supplied by
release CI. `omasheets-window --provenance` and
`omasheets-lok-render --provenance` expose that identity; the Arch production
installation job verifies it against the installed checkout.

The native window provides scrolling, selection, keyboard editing, sheet
switching, formula/address feedback, zoom, undo/redo, copy/paste, formatting,
dirty-close handling, Save a Copy, the private live-document bridge and the
human-controlled diff overlay. `.xlsx` and `.ods` are editable; `.xls` and
`.xlsm` remain read-only.

CI creates its workbook fixtures from the historical files under
`spikes/libreofficekit/fixtures/`, but no production command runs a build-tree
executable.

LibreOfficeKit's API is unstable and real Omarchy/Wayland acceptance remains a
separate gate. See `docs/ACCEPTANCE.md` for the hands-on evidence still needed.
