# OmaSheets Rust migration preview — 5 October 2026

This is an isolated test bundle for x86_64 Arch/Omarchy. It does not install or
replace OmaSheets. The normal desktop package still contains Python. Session
review/UI, the full compatibility CLI/MCP, installation and tooling migration
remain incomplete, so this is not v0.1.0 or evidence of full Python removal.

After a normal full system update, the runtime requires `libreoffice-fresh`,
`bubblewrap` and glibc. The normal OmaSheets package supplies these dependencies.
No Rust compiler, LibreOffice SDK or Python interpreter is needed by the bundle.
`BUILD_PACKAGES` records the tested Arch versions; `SOURCE_COMMIT` records the
exact source revision. These are unsigned development assets.

## Run the automated checks

Download the archive and its `.sha256` file into an empty directory, then run:

```bash
sha256sum --check omasheets-rust-preview-linux-x86_64.tar.gz.sha256
tar -xzf omasheets-rust-preview-linux-x86_64.tar.gz
cd omasheets-rust-preview-linux-x86_64
set -o pipefail
bash test.sh 2>&1 | tee ../omasheets-rust-xps-test.log
```

The script checks bundled file hashes, UNO dispatch, real native-service
lifecycle/agent contracts, and sandboxed LibreOffice operations using generated
ODS/XLSX/XLS fixtures. It uses disposable temporary documents and does not need
your workbooks. Success ends with `PASS: Rust native and sandboxed LibreOffice
acceptance`. The native test uses a grid stand-in; it is not visual acceptance.
The compatibility runner exercises worker/library APIs, not the unfinished
end-to-end compatibility review interface. Keep the terminal log if it fails.
Do not disable Bubblewrap to work around a failure.

## Optional native window check

After installing the matching OmaSheets desktop package, close its windows.
From this extracted directory:

```bash
OMASHEETS_GRID=/usr/lib/omasheets/bin/omasheets-grid ./omasheets launch
```

The Rust supervisor finds the bundled native service beside itself. Check native
creation/edit/save/reopen and two windows closing independently. Compatibility
import and desktop helper paths still belong to the production implementation;
this window check does not establish a Python-free installed application.

To remove this preview, close its windows and delete the extracted directory.
Keep any documents you created. No package removal or system configuration change
is performed by the preview script.
