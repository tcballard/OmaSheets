#!/usr/bin/env bash
set -euo pipefail
preview_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cd "$preview_dir"
sha256sum --check SHA256SUMS
export OMASHEETS_APP="$preview_dir/omasheets"
export OMASHEETS_UNO_BRIDGE="$preview_dir/omasheets-uno-bridge"
export SAL_USE_VCLPLUGIN=svp
preview_commit=$(cat SOURCE_COMMIT)
[[ "$preview_commit" =~ ^[0-9a-f]{40}$ ]] || { printf 'Invalid source revision\n' >&2; exit 1; }
preview_provenance=$(./omasheets --provenance)
[[ "$preview_provenance" == *"\"source_commit\":\"$preview_commit\""* ]] || { printf 'Executable source revision does not match bundle\n' >&2; exit 1; }
printf 'Rust preview source: %s\n' "$preview_commit"
printf '%s\n' "$preview_provenance"
./omasheets-uno-bridge --self-test
timeout 120 ./examples/native_acceptance
timeout 300 ./examples/compatibility_acceptance
printf '\nPASS: Rust native and sandboxed LibreOffice acceptance. Visual desktop testing remains separate.\n'
