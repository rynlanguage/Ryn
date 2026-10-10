"""Port of the Rust-oracle coverage in tests/ryn_sema_ratchet.rs: `rync check` must report the recorded diagnostics.

tests/fixtures/ryn_sema_coverage.tsv records, per invalid program, the diagnostic that the Rust analyzer produced
(columns: class, fixture path, rust code, rust message). The Rust analyzer is going away, so the recorded diagnostic is
now the expectation itself: the first diagnostic of `rync check --stdlib <stdlib> <fixture>` must have the recorded code
and message. Project fixtures (`<project>/src/main.ryn`) are checked as their project directory.

A row that differs is a compiler gap, listed in tools/tests/DIAGNOSTIC_GAPS.md. The assertion is never weakened.
"""

import os
import re
import subprocess
import unittest
from pathlib import Path

from ryn_harness import IS_WINDOWS, REPO_ROOT, ryn_binary

COVERAGE = REPO_ROOT / "tests" / "fixtures" / "ryn_sema_coverage.tsv"
STDLIB = REPO_ROOT / "stdlib" / "std" / "src"
DIAGNOSTIC = re.compile(r"^error\[(R\d{4})\]: (.*)$")
TIMEOUT_SECONDS = 120


def rync_binary() -> Path:
    override = os.environ.get("RYN_TEST_RYNC")
    if override:
        return Path(override)
    return Path(ryn_binary()).with_name("rync" + (".exe" if IS_WINDOWS else ""))


def coverage_rows():
    """The rows of the coverage fixture, as (class, fixture path, rust code, rust message)."""
    rows = []
    for line in COVERAGE.read_text(encoding="utf-8").splitlines():
        if not line:
            continue
        fields = line.split("\t", 3)
        fields += [""] * (4 - len(fields))
        rows.append(tuple(fields))
    return rows


def check_target(fixture: str) -> Path:
    """The path `rync check` receives: a project's directory for `<project>/src/main.ryn`, else the file."""
    path = REPO_ROOT / fixture
    if path.parent.name == "src" and path.name == "main.ryn":
        return path.parent.parent
    return path


def comparable_message(message: str, recorded: str) -> str:
    """The message up to its path, when the recorded message has one: such paths are machine-specific.

    Both the recorded and the actual message are cut at the first absolute path that starts a
    word or a parenthesis, so `... (looked for <path>)` compares as `... (looked for`.
    """
    if not re.search(r"[ (]/", recorded):
        return message
    path = re.search(r"[ (]/", message)
    return (message[: path.start()] if path else message).rstrip()


def first_diagnostic(stderr: str):
    """The (code, message) of the first `error[CODE]: message` line, or None."""
    for line in stderr.splitlines():
        match = DIAGNOSTIC.match(line)
        if match:
            return match.group(1), match.group(2)
    return None


class DiagnosticCorpusTests(unittest.TestCase):
    def test_check_reports_the_recorded_rust_diagnostic_for_every_invalid_program(self):
        rync = rync_binary()
        if not rync.is_file():
            self.fail(f"rync not found at {rync}; set RYN_TEST_RYNC")

        rows = [row for row in coverage_rows() if row[2]]
        self.assertTrue(rows, "the coverage fixture must record invalid programs")

        for _, fixture, code, message in rows:
            with self.subTest(fixture=fixture):
                target = check_target(fixture)
                result = subprocess.run(
                    [str(rync), "check", "--stdlib", str(STDLIB), str(target)],
                    capture_output=True,
                    timeout=TIMEOUT_SECONDS,
                )
                stderr = result.stderr.decode("utf-8", errors="replace")
                self.assertNotEqual(
                    result.returncode,
                    0,
                    f"{fixture} must be rejected by `rync check`",
                )
                first = first_diagnostic(stderr)
                self.assertIsNotNone(
                    first, f"{fixture}: no diagnostic in stderr:\n{stderr}"
                )
                actual_code, actual_message = first
                self.assertEqual(actual_code, code, f"{fixture}: diagnostic code")
                self.assertEqual(
                    comparable_message(actual_message, message),
                    comparable_message(message, message),
                    f"{fixture}: diagnostic message (actual: {actual_message!r})",
                )


if __name__ == "__main__":
    unittest.main()
