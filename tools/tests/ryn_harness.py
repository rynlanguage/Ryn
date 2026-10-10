"""Shared helpers for the cargo-free Ryn CLI tests.

The `ryn` binary under test is taken from the RYN_TEST_BIN environment
variable, which tools/run-tests.py sets from its positional argument.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = REPO_ROOT / "examples"
PROGRAMS = REPO_ROOT / "tests" / "programs"
IS_WINDOWS = os.name == "nt"


@dataclass
class Run:
    code: int | None
    stdout: str
    stderr: str


def ryn_binary() -> str:
    path = os.environ.get("RYN_TEST_BIN")
    if not path:
        raise RuntimeError(
            "RYN_TEST_BIN is not set; run tools/run-tests.py <ryn-binary>"
        )
    return path


def run_ryn(*args: object, cwd: Path | str | None = None, env: dict | None = None) -> Run:
    """Runs the ryn binary and captures its output, like `Command::output()`."""
    full_env = os.environ.copy()
    if env:
        full_env.update(env)
    proc = subprocess.run(
        [ryn_binary(), *(str(arg) for arg in args)],
        cwd=None if cwd is None else str(cwd),
        env=full_env,
        capture_output=True,
    )
    return Run(
        proc.returncode,
        proc.stdout.decode("utf-8", errors="replace"),
        proc.stderr.decode("utf-8", errors="replace"),
    )


def expected_version() -> str:
    """The version the binary must report: RYN_EXPECTED_VERSION, else the repo's Cargo.toml."""
    override = os.environ.get("RYN_EXPECTED_VERSION")
    if override:
        return override
    text = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
    if not match:
        raise RuntimeError("could not find the package version; set RYN_EXPECTED_VERSION")
    return match.group(1)


def unique_suffix() -> str:
    return f"{os.getpid()}-{time.time_ns()}"


class TempDir:
    """A fresh directory removed on exit, mirroring the Rust `TestDirectory` helper."""

    def __init__(self, label: str):
        self.path = Path(tempfile.mkdtemp(prefix=f"ryn-{label}-{unique_suffix()}-"))

    def __enter__(self) -> Path:
        return self.path

    def __exit__(self, *exc: object) -> None:
        shutil.rmtree(self.path, ignore_errors=True)
