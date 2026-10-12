"""The AArch64 backend runs the language suite it accepts as the x86-64 backend does.

Every `// test: run` program of tests/suite (Linux, no stdin) is compiled with `rync --target aarch64-linux`. A program the
backend refuses is counted and named ("the AArch64 backend does not handle ..."); a program it accepts is run under the
Unicorn emulator (tools/tests/arm64_unicorn.py) and its standard output and exit status must match the `// out:` and
`// exit:` headers. A mismatch, a compile error for a program the backend should accept, or an emulation fault fails the
test. Emulation needs the `unicorn` package and the compiler needs RYN_TEST_RYNC; without either the test is skipped.
"""

import concurrent.futures
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from ryn_harness import REPO_ROOT
import arm64_unicorn

SUITE = REPO_ROOT / "tests" / "suite"
STDLIB = REPO_ROOT / "stdlib" / "std" / "src"
REFUSED = "the AArch64 backend does not handle"


def header(path):
    case = {"test": None, "out": [], "exit": None, "target": None, "stdin": False}
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.startswith("//"):
            break
        rest = line[2:].removeprefix(" ")
        if ":" not in rest:
            continue
        key, value = rest.split(":", 1)
        value = value.removeprefix(" ")
        if key == "test":
            case["test"] = value.strip()
        elif key == "out":
            case["out"].append(value)
        elif key == "exit":
            case["exit"] = int(value.strip())
        elif key == "target":
            case["target"] = value.strip()
        elif key == "in":
            case["stdin"] = True
    return case


def runnable_programs():
    """Every suite program that is run (not rejected) on Linux, without stdin."""
    programs = []
    for category in sorted(p for p in SUITE.iterdir() if p.is_dir()):
        for child in sorted(category.iterdir()):
            if child.is_file() and child.suffix == ".ryn":
                case = header(child)
                if case["test"] == "run" and case["target"] != "windows" and not case["stdin"]:
                    programs.append((child, case))
    return programs


def compile_for_arm64(rync, source, directory):
    """The executable path, or the compiler's message when the backend refuses or fails."""
    exe = os.path.join(directory, "program")
    done = subprocess.run(
        [rync, "--target", "aarch64-linux", "--stdlib", str(STDLIB), str(source), "-o", exe],
        capture_output=True,
        text=True,
        timeout=600,
    )
    if done.returncode == 0:
        return exe, None
    return None, done.stderr + done.stdout


def expected_stdout(case):
    return "".join(line + "\n" for line in case["out"]).encode("utf-8")


def compile_all(rync, programs):
    """Compiles every program in parallel (the compiler runs as a separate process for each one)."""
    tmp = tempfile.mkdtemp(prefix="arm64-run-")
    jobs = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        for index, (path, case) in enumerate(programs):
            directory = os.path.join(tmp, str(index))
            os.makedirs(directory)
            jobs[index] = (path, case, pool.submit(compile_for_arm64, rync, path, directory))
        results = {index: (path, case, future.result()) for index, (path, case, future) in jobs.items()}
    return results


class Arm64Suite(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rync = os.environ.get("RYN_TEST_RYNC")
        if not arm64_unicorn.available():
            raise unittest.SkipTest("the unicorn package is not installed")
        if not cls.rync:
            raise unittest.SkipTest("RYN_TEST_RYNC is not set")

    def test_accepted_programs_match_the_x86_suite(self):
        programs = runnable_programs()
        results = compile_all(self.rync, programs)
        accepted = 0
        refused = []
        failures = []
        for index in sorted(results):
            path, case, (exe, message) = results[index]
            name = str(path.relative_to(REPO_ROOT))
            if exe is None:
                if REFUSED in message:
                    refused.append(name)
                else:
                    failures.append(f"{name}: compile error: {message.strip()[:300]}")
                continue
            accepted += 1
            with open(exe, "rb") as handle:
                data = handle.read()
            # Instructions are checked one by one first; a program too long for that is run at full speed instead.
            outcome = arm64_unicorn.run(data, limit=5_000_000)
            if outcome.error and "more than" in outcome.error:
                outcome = arm64_unicorn.run(data, alignment=False, timeout_us=900_000_000)
            want_exit = 0 if case["exit"] is None else case["exit"]
            problems = []
            if outcome.error:
                problems.append(outcome.error)
            if outcome.stdout != expected_stdout(case):
                problems.append(f"stdout {outcome.stdout[:120]!r} != {expected_stdout(case)[:120]!r}")
            if outcome.status != want_exit:
                problems.append(f"exit {outcome.status} != {want_exit}")
            if problems:
                failures.append(f"{name}: " + "; ".join(problems))
        print(f"\nAArch64 backend: accepted {accepted} of {len(programs)} run programs, refused {len(refused)}")
        self.assertEqual(failures, [], "accepted programs that do not run as the x86-64 backend runs them")
        self.assertGreaterEqual(accepted, 60, "the backend should accept at least the scalar programs of the suite")


class Arm64Programs(unittest.TestCase):
    """Small programs with known output, compiled and run under the emulator."""

    @classmethod
    def setUpClass(cls):
        cls.rync = os.environ.get("RYN_TEST_RYNC")
        if not arm64_unicorn.available():
            raise unittest.SkipTest("the unicorn package is not installed")
        if not cls.rync:
            raise unittest.SkipTest("RYN_TEST_RYNC is not set")

    def run_source(self, source, alignment=True):
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "main.ryn")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(source)
            exe, message = compile_for_arm64(self.rync, path, directory)
            self.assertIsNotNone(exe, message)
            with open(exe, "rb") as handle:
                return arm64_unicorn.run(handle.read(), alignment=alignment)

    def test_integers_bool_and_division_by_zero(self):
        outcome = self.run_source("fun main() {\n    echo 42\n    echo -7\n    echo 1 + 2\n    echo 7 / 2\n    echo 7 % 3\n    echo 3 < 4\n}\n")
        self.assertEqual(outcome.stdout, b"42\n-7\n3\n3\n1\ntrue\n")
        self.assertEqual(outcome.status, 0)
        outcome = self.run_source("fun zero(d: i64) -> i64 {\n    return 5 / d\n}\nfun main() {\n    echo zero(0)\n}\n")
        self.assertEqual(outcome.stdout, b"")
        self.assertEqual(outcome.stderr, b"Ryn runtime error: integer division or remainder by zero\n")
        self.assertEqual(outcome.status, 1)

    def test_narrow_integers_wrap_as_on_x86(self):
        outcome = self.run_source("fun main() {\n    a: u8 = 250\n    echo a + 10\n    b: i8 = 127\n    echo b + 1\n    c: i32 = 2147483647\n    echo c + 1\n}\n")
        self.assertEqual(outcome.stdout, b"4\n-128\n-2147483648\n")

    def test_stack_overflow_reports_and_exits(self):
        outcome = self.run_source("fun deep(n: i64) -> i64 {\n    return 1 + deep(n + 1)\n}\nfun main() {\n    echo deep(0)\n}\n", alignment=False)
        self.assertEqual(outcome.status, 1)
        self.assertEqual(outcome.stderr, b"Ryn runtime error: stack overflow (recursion is too deep)\n")


if __name__ == "__main__":
    unittest.main()
