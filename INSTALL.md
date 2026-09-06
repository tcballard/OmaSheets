# Install OmaSheets on Omarchy

Close any OmaSheets windows, then install or update the development build:

```bash
curl -fsSL https://raw.githubusercontent.com/tcballard/OmaSheets/main/bin/omasheets-install | bash
```

Open **OmaSheets** from the application launcher, or run `omasheets`.
Future updates use:

```bash
omasheets update
```

The command finds the newest published development build, downloads its native
archive and checksum, fetches the matching source into a temporary checkout,
and runs the existing installer. You do not need GitHub login, a manually
saved ZIP, a revision number, a compiler, or an Omarchy plugin checkout.
Existing installations are updated with rollback on failure; workbooks are
preserved. Linux x86_64 is currently supported. The helper uses curl, jq, git,
sha256sum and Omarchy's system Python.

If runtime dependencies are missing, the installer prints the required command:

```bash
omarchy pkg add gtk3 libreoffice-fresh bubblewrap qt6-base qt6-declarative qt6-wayland
```

Run that command, then repeat the installer. It never installs system packages
or requests privilege itself. Close native windows before updating. If you
explicitly enabled the optional systemd service, stop it before updating and
restart it afterwards.

## Optional Omarchy bar widget

```bash
omarchy plugin add https://github.com/tcballard/OmaSheets.git --enable
```

The widget's **Install OmaSheets** action uses the same development installer.
The standalone application does not require the widget. Omarchy does not run
plugin install hooks.

## Build channels and verification

**Development builds** are public GitHub prereleases tagged `dev-<commit>`.
Automation publishes them only after the complete main-branch CI workflow
passes, including compiler-free Arch installation. Draft releases are hidden
until both archive and checksum are uploaded. These builds rely on GitHub's
repository and CI access controls; they are not maintainer-signed production
releases. The installer verifies the checksum, source commit, tracked-source
digest, platform, version, allow-listed payload and executable provenance.
It leaves your plugin or development checkout unchanged.

**Production releases** retain the separate signed installation path described
in [RELEASE.md](docs/RELEASE.md): an exact version tag, the offline maintainer's
pinned public key, a valid detached `.minisig`, checksum and provenance are all
required. The development channel does not satisfy or remove those
[v0.1.0 release gates](docs/V0.1-RELEASE.md). The older v0.0.2 release does not
contain the current native app.

An explicit source-matching local bundle can still be installed with
`OMASHEETS_NATIVE_BUNDLE_PATH` through `scripts/install.py install`. That path
remains useful for contributors and CI.

## Installed surfaces

The bootstrap installs these surfaces together:

- Python package and native binaries under
  `$XDG_DATA_HOME/omasheets/app/` (normally `~/.local/share/omasheets/app/`);
- the stable `~/.local/bin/omasheets` launcher;
- the source-bound `omasheets-grid` Qt executable for native documents;
- the Codex plugin under `~/.codex/plugins/omasheets/`, with an absolute MCP
  command and a personal marketplace entry in
  `~/.agents/plugins/marketplace.json`;
- the desktop entry under `$XDG_DATA_HOME/applications/`;
- the native MIME declaration under `$XDG_DATA_HOME/mime/packages/`; and
- OmaSheets MIME associations in `$XDG_CONFIG_HOME/mimeapps.list`.

Private workbook state, receipts and installation journals live under
`$XDG_STATE_HOME/omasheets/`; the verified release download is cached under
`$XDG_CACHE_HOME/omasheets/`; sockets and live snapshots live under
`$XDG_RUNTIME_DIR/omasheets/`. Runtime and state directories are mode `0700`.

The native bundle targets Omarchy's Linux `x86_64` platform.
Installation fails before changing product state on any architecture without a
matching release bundle.

Verify the result:

```bash
omasheets doctor
omasheets --version
```

`doctor` must report Bubblewrap, LibreOffice, Python UNO, the compatibility
window, native Qt grid and desktop integration. The Omarchy bar plugin is optional. Restart or refresh Codex after the
first installation so it discovers the new personal plugin and MCP server.

Launch **OmaSheets** from the app menu, or run `omasheets`. Use **New workbook**
(Ctrl+N) and choose a `.omasheets` filename. Type a value or formula, then press
Enter or Ctrl+S to save the cell. Close and reopen with **Open workbook**
(Ctrl+O) to continue. F1 shows the keyboard guide. The File menu also provides
Excel import and XLSX, CSV or Parquet export, with a report of conversion limits.

Open or select a compatibility workbook, then choose **Ask Agent** from either the
compatibility window header or Omarchy bar. OmaSheets passes a fixed path-free prompt to
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
