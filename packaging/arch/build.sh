#!/bin/bash
# Prepare a verified system payload and an AUR recipe. Run makepkg as a user.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
archive=$(realpath "$1")
output=$(realpath -m "$2")
mkdir -p "$output"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
commit=$(git rev-parse HEAD)
version="0.0.2.r$(git show -s --format=%ct HEAD).g${commit:0:12}"
export SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(git show -s --format=%ct HEAD)

# Reuse the existing allowlist, checksum and executable provenance verifier.
PYTHONPATH="$root/src" python - "$root" "$archive" "$stage" <<'PY'
import json
import shutil
import sys
from pathlib import Path
from omasheets import __version__
from omasheets.installation import source_identity, _launcher
from omasheets.integration import DESKTOP_ENTRY, DESKTOP_ID, MIME_PACKAGE
from omasheets.native_bundle import install_native_bundle

root, archive, stage = map(Path, sys.argv[1:])
app = stage / 'usr/lib/omasheets'
identity = source_identity(root)
manifest = install_native_bundle(archive, app, version=__version__, source=identity)
# Package updates belong to Pacman. Never ship the home-directory setup here.
(app / 'bin/omasheets-setup').unlink()
shutil.copytree(root / 'src/omasheets', app / 'lib/omasheets',
                ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
(app / 'package-manager').write_text('pacman\n')
(app / 'provenance.json').write_text(json.dumps({'source': identity, 'native_bundle': manifest}) + '\n')
bin_dir = stage / 'usr/bin'
bin_dir.mkdir(parents=True)
(bin_dir / 'omasheets').write_bytes(_launcher(Path('/usr/lib/omasheets')))
(bin_dir / 'omasheets').chmod(0o755)
(bin_dir / 'omasheets-service').symlink_to('../lib/omasheets/bin/omasheets-service')
desktop = stage / 'usr/share/applications' / DESKTOP_ID
desktop.parent.mkdir(parents=True)
desktop.write_text(DESKTOP_ENTRY.replace('Exec=omasheets ', 'Exec=/usr/bin/omasheets '))
mime = stage / 'usr/share/mime/packages/io.github.tcballard.OmaSheets.xml'
mime.parent.mkdir(parents=True)
mime.write_bytes(MIME_PACKAGE)
license = stage / 'usr/share/licenses/omasheets-bin/LICENSE'
license.parent.mkdir(parents=True)
shutil.copyfile(root / 'LICENSE', license)
PY
payload="omasheets-runtime-$version-x86_64.tar.gz"
tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
  -C "$stage" -czf "$output/$payload" usr
checksum=$(sha256sum "$output/$payload" | cut -d' ' -f1)
sed -e "s/@VERSION@/$version/g" -e "s/@COMMIT@/$commit/g" -e "s/@CHECKSUM@/$checksum/g" \
  "$root/packaging/arch/PKGBUILD.in" > "$output/PKGBUILD"
(cd "$output" && sha256sum "$payload" > "$payload.sha256")
printf 'Prepared %s and PKGBUILD in %s\n' "$payload" "$output"
