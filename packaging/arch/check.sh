#!/bin/bash
# CI integration check; requires a disposable Arch container and builder user.
set -euo pipefail
directory=$1
packages=("$directory"/*.pkg.tar.zst)
test "${#packages[@]}" = 1
# Start with the old home-directory installation to test a real migration.
runuser -u builder -- env GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory \
  GIT_CONFIG_VALUE_0="$PWD" OMASHEETS_NATIVE_BUNDLE_PATH="$PWD/dist-native/omasheets-native-0.0.2-linux-x86_64.tar.gz" \
  python scripts/install.py install
pacman -U --noconfirm "${packages[0]}"
runuser -u builder -- /usr/bin/omasheets migrate-user-install
test ! -e /home/builder/.local/bin/omasheets
test ! -e /home/builder/.local/share/omasheets/app
pacman -Qk omasheets-bin
desktop-file-validate /usr/share/applications/io.github.tcballard.OmaSheets.desktop
test ! -e /usr/lib/omasheets/bin/omasheets-setup
test ! -e /usr/lib/omasheets/bin/omasheets-update
install -d -m 700 -o builder -g builder /tmp/omasheets-package-runtime
runuser -u builder -- env XDG_RUNTIME_DIR=/tmp/omasheets-package-runtime bash -eu <<'SH'
omasheets doctor --json > /tmp/omasheets-package-runtime/doctor.json
omasheets update
mkdir -p "$HOME/Documents"
printf 'keep my workbook\n' > "$HOME/Documents/keep.omasheets"
xvfb-run -a env OMASHEETS_UI_CAPTURE=/tmp/omasheets-package-runtime/welcome.png timeout 30 omasheets
test -s /tmp/omasheets-package-runtime/welcome.png
SH
python -c 'import json; assert json.load(open("/tmp/omasheets-package-runtime/doctor.json"))["ready"]'
mkdir -p /tmp/omasheets-package-runtime/service
chown builder:builder /tmp/omasheets-package-runtime/service
runuser -u builder -- python scripts/check_native_service_install.py \
  --service /usr/bin/omasheets-service --workdir /tmp/omasheets-package-runtime/service
# Exercise a higher pkgrel through Pacman, using the same tested payload.
runuser -u builder -- bash -eu -c 'cd "$1"; sed -i "s/^pkgrel=1$/pkgrel=2/" PKGBUILD; makepkg --noconfirm' bash "$directory"
upgrades=("$directory"/*-2-x86_64.pkg.tar.zst)
pacman -U --noconfirm "${upgrades[0]}"
pacman -Qk omasheets-bin
pacman -R --noconfirm omasheets-bin
test ! -e /usr/bin/omasheets
test ! -e /usr/share/applications/io.github.tcballard.OmaSheets.desktop
test ! -e /usr/share/mime/packages/io.github.tcballard.OmaSheets.xml
test ! -e /usr/lib/omasheets
runuser -u builder -- test -s /home/builder/Documents/keep.omasheets
# Publish only the original tested pkgrel and its corresponding recipe.
rm "${upgrades[0]}"
sed -i 's/^pkgrel=2$/pkgrel=1/' "$directory/PKGBUILD"
