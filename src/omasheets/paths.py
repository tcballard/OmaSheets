"""XDG state paths with private-directory creation."""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True, slots=True)
class AppPaths:
    state: Path
    cache: Path
    runtime: Path

    @classmethod
    def discover(cls) -> "AppPaths":
        user_home = Path.home()
        state = Path(os.environ.get("XDG_STATE_HOME", user_home / ".local/state")) / "omasheets"
        cache = Path(os.environ.get("XDG_CACHE_HOME", user_home / ".cache")) / "omasheets"
        runtime_root = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
        return cls(state=state, cache=cache, runtime=runtime_root / "omasheets")

    def ensure(self) -> None:
        for directory in (self.state, self.cache, self.runtime):
            directory.mkdir(mode=0o700, parents=True, exist_ok=True)
            directory.chmod(0o700)

