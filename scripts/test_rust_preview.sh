#!/usr/bin/env bash
set -euo pipefail
preview_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cd "$preview_dir"
sha256sum --check SHA256SUMS
export OMASHEETS_APP="$preview_dir/omasheets"
export OMASHEETS_UNO_BRIDGE="$preview_dir/omasheets-uno-bridge"
export SAL_USE_VCLPLUGIN=svp
printf 'Rust preview source: '
cat SOURCE_COMMIT
./omasheets --provenance
./omasheets-uno-bridge --self-test
timeout 120 ./examples/native_acceptance
timeout 300 ./examples/compatibility_acceptance
printf '\nPASS: Rust native and sandboxed LibreOffice acceptance. Visual desktop testing remains separate.\n'
