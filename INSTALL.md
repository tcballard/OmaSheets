# Install OmaSheets v0.0.2 on Omarchy

OmaSheets targets Omarchy `quattro`. Install and enable its Omarchy surface
from the repository with:

```bash
omarchy plugin add https://github.com/tcballard/OmaSheets.git --enable
```

That command is the complete Omarchy plugin installation. Omarchy clones and
validates the repository, adds the bar widget, and deliberately does not run
plugin install hooks or `sudo`.

Open the OmaSheets bar widget and choose **Install OmaSheets** to finish the
user-local product bootstrap. The same action can be run in a terminal:

```bash
~/.config/omarchy/plugins/io.github.tcballard.omasheets/bin/omasheets-plugin install
```

The bootstrap checks runtime dependencies, downloads the native bundle from the
matching `v0.0.2` GitHub release, verifies it against the exact installed
checkout, and installs only OmaSheets-owned files. No compiler, CMake,
`pkgconf`, or LibreOffice SDK is installed or required on the user's machine.
The bootstrap never invokes a package manager or requests privilege. If runtime
dependencies are missing, it stops and prints this explicit Omarchy command for
the user to approve and run:

```bash
omarchy pkg add gtk3 libreoffice-fresh bubblewrap
```

Automatic bundle download additionally requires checkout `HEAD` to be exactly
the matching `v<version>` tag. A source checkout ahead of the published release
fails before downloading anything. CI and development builds can instead set
`OMASHEETS_NATIVE_BUNDLE_PATH` to an explicit bundle built from that exact
checkout; the normal source-identity and executable-provenance checks still
apply.

Current Arch `libreoffice-fresh` ships the system Python `uno` module and
`libpyuno`; there is no separate `python-uno` package. The bootstrap still
checks `import uno` explicitly. Then run the **Install OmaSheets** action again.

## Installed surfaces

The bootstrap installs these surfaces together:

- Python package and native binaries under
  `$XDG_DATA_HOME/omasheets/app/` (normally `~/.local/share/omasheets/app/`);
- the stable `~/.local/bin/omasheets` launcher;
- the Codex plugin under `~/.codex/plugins/omasheets/`, with an absolute MCP
  command and a personal marketplace entry in
  `~/.agents/plugins/marketplace.json`;
- the desktop entry under `$XDG_DATA_HOME/applications/`; and
- OmaSheets MIME associations in `$XDG_CONFIG_HOME/mimeapps.list`.

Private workbook state, receipts and installation journals live under
`$XDG_STATE_HOME/omasheets/`; the verified release download is cached under
`$XDG_CACHE_HOME/omasheets/`; sockets and live snapshots live under
`$XDG_RUNTIME_DIR/omasheets/`. Runtime and state directories are mode `0700`.

The v0.0.2 native bundle targets Omarchy's Linux `x86_64` platform.
Installation fails before changing product state on any architecture without a
matching release bundle.

Verify the result:

```bash
omasheets doctor
omasheets --version
```

`doctor` must report Bubblewrap, LibreOffice, Python UNO, the native window,
desktop integration and the Omarchy plugin. Restart or refresh Codex after the
first installation so it discovers the new personal plugin and MCP server.

Open or select a workbook, then choose **Ask Agent** from either the native
window header or Omarchy bar. OmaSheets passes a fixed path-free prompt to
`omarchy agent prompt`, which launches the default agent selected in Omarchy.
The Codex plugin supplies native MCP discovery when Codex is that default; other
agents can use their own MCP configuration or the prompt's provider-neutral
`omasheets agent-session` JSON command bridge. If the Omarchy launcher is not
on `PATH`, the command reports that the agent entry point is unavailable while
spreadsheet editing remains functional.

## Removal

Remove the product-owned files before removing the Omarchy checkout:

```bash
~/.local/bin/omasheets uninstall
omarchy plugin remove io.github.tcballard.omasheets
```

The uninstall journal removes only files whose content still matches what
OmaSheets installed. Modified launchers, desktop entries, Codex plugin files and
associations are preserved and reported as conflicts. Unrelated MIME entries,
personal marketplace plugins and Codex plugin directories are retained.

Omarchy itself has no uninstall hook, so `omarchy plugin remove` alone removes
only the bar-plugin checkout. If that happened first, the installed
`~/.local/bin/omasheets uninstall` command remains available for cleanup.
