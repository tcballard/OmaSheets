"""Run a helper with producer-side output and time limits.

The Omarchy bar widget reads the helper's stdout through a collector that can
only judge the output after it has all arrived. The limits therefore have to be
enforced here, on the producer side: reading stops the moment the byte limit is
exceeded, a hard deadline covers the whole run, and the helper's entire process
group receives SIGTERM and then SIGKILL when either limit trips.
"""

from __future__ import annotations

import os
import select
import signal
import subprocess
import time
from dataclasses import dataclass
from typing import Sequence


@dataclass(frozen=True, slots=True)
class BoundedResult:
    status: str
    output: bytes
    returncode: int | None

    @property
    def ok(self) -> bool:
        return self.status == "ok" and self.returncode == 0


def terminate_process_group(process: subprocess.Popen, grace_seconds: float = 1.0) -> None:
    """Send SIGTERM to the helper's process group, then SIGKILL, and reap it."""
    group = process.pid
    for requested in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(group, requested)
        except ProcessLookupError:
            pass
        except PermissionError:
            process.kill()
        try:
            process.wait(timeout=grace_seconds)
            break
        except subprocess.TimeoutExpired:
            continue
    if process.poll() is None:
        process.kill()
        process.wait()


def run_bounded(
    command: Sequence[str],
    *,
    byte_limit: int,
    timeout_seconds: float,
    grace_seconds: float = 1.0,
) -> BoundedResult:
    """Run ``command`` in its own session with output and deadline limits.

    ``status`` is ``"ok"`` when the helper exited within the deadline and wrote
    at most ``byte_limit`` bytes, ``"overflow"`` when it wrote more (reading
    stops at the first byte past the limit), or ``"timeout"`` when the deadline
    passed. Overflow and timeout always end with the process group terminated.
    """
    if byte_limit <= 0 or timeout_seconds <= 0:
        raise ValueError("byte_limit and timeout_seconds must be positive")
    deadline = time.monotonic() + timeout_seconds
    process = subprocess.Popen(
        list(command),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
        close_fds=True,
    )
    assert process.stdout is not None
    descriptor = process.stdout.fileno()
    chunks: list[bytes] = []
    collected = 0
    status = "ok"
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                status = "timeout"
                break
            ready, _, _ = select.select([descriptor], [], [], remaining)
            if not ready:
                status = "timeout"
                break
            chunk = os.read(descriptor, min(65536, byte_limit + 1 - collected))
            if not chunk:
                break
            chunks.append(chunk)
            collected += len(chunk)
            if collected > byte_limit:
                status = "overflow"
                break
        if status == "ok":
            try:
                process.wait(timeout=max(0.0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                status = "timeout"
    finally:
        if status != "ok" or process.poll() is None:
            terminate_process_group(process, grace_seconds)
        process.stdout.close()
    output = b"".join(chunks)
    return BoundedResult(status=status, output=output[:byte_limit], returncode=process.returncode)
