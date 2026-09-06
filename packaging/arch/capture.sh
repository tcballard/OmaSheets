#!/bin/bash
# The desktop launcher returns immediately; keep Xvfb alive for its child.
set -euo pipefail
/usr/bin/omasheets
for _ in {1..200}; do
  if [[ -s $OMASHEETS_UI_CAPTURE && ! -S $XDG_RUNTIME_DIR/omasheets/native.sock ]]; then
    exit 0
  fi
  sleep 0.1
done
echo 'The installed window did not render and shut down within 20 seconds.' >&2
exit 1
